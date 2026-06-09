# Storage Fine Tuning

This page is for sysadmins who want to tune **where** OxiCloud spools
upload bodies and **why** the placement matters for throughput and
memory. The defaults work; the gains from a tuned layout are
significant on busy instances or constrained containers.

## The upload lifecycle in 30 seconds

Every upload moves through two stages:

```
        ┌─── direct (single-PUT) upload ────────┐
client ─┤                                       ├──► OxiCloud accepts the
        └─── multi-chunk upload                 │    bytes into a SPOOL on
             (`/api/uploads` /                  │    local disk.
              `/dav/uploads/...`)               │
                                                │    Direct upload  → OXICLOUD_UPLOAD_TMPDIR
                                                │    Chunked upload → OXICLOUD_CHUNK_DIR
                                                │
                                                ▼
                                  ┌─────────────────────────┐
                                  │ Once the upload is      │
                                  │ complete (and verified  │
                                  │ if a checksum was       │
                                  │ supplied), OxiCloud     │
                                  │ MOVES the assembled     │
                                  │ blob into the configured│
                                  │ STORAGE BACKEND:        │
                                  │                         │
                                  │  • local FS (.blobs/)   │
                                  │  • S3-compatible        │
                                  │  • Azure Blob           │
                                  └─────────────────────────┘
```

Two practical consequences:

- **The spool/chunk directories see write-heavy churn** during uploads —
  fast disk (NVMe) and sufficient free space matter more here than on
  the final storage backend.
- **The promotion from spool → storage is a `rename(2)` whenever
  source and destination share a filesystem** (i.e. when the backend
  is `local` and the spool dir is on the same FS as `.blobs/`). On
  remote backends (S3, Azure) the promotion is always a network
  upload from the local spool; placement of the spool still matters
  for intake throughput but the "same FS" rule doesn't apply.

## Upload size caps — what each one bounds

Three independent caps control how large an upload OxiCloud will
accept. Pick them with disk and tmpfs sizing in mind: the spool/chunk
directories must be able to hold the worst case (max cap × concurrent
uploads).

| Variable | Default | What it caps |
|---|---|---|
| `OXICLOUD_MAX_UPLOAD_SIZE` | 10 GB | Total file size — whether uploaded as one `PUT` or assembled from many chunks. The hard ceiling on any single file. |
| `OXICLOUD_CHUNK_MAX_BYTES` | 100 MB | A single chunked-PUT request body (`PATCH /api/uploads/{id}` or `PUT /dav/uploads/.../chunk`). Independent of the whole-file cap — a 5 GB file uploaded in 100 MB chunks needs ~50 PUTs, each bounded by this. |
| `OXICLOUD_MAX_UPLOAD_SIZE` (again, for the PUT path) | 10 GB | A single non-chunked PUT body. Same env var as the whole-file cap because for non-chunked uploads they're equivalent. |

### Why these matter for tmpfs sizing

OxiCloud streams bodies frame-by-frame, so **RAM** is bounded to one
HTTP frame (~64 KB) per request regardless of the caps. **Disk space**,
however, scales with the caps:

- **Direct PUT** (single-file): each in-flight upload spools the full
  body to disk under `OXICLOUD_UPLOAD_TMPDIR` until promotion. Worst
  case disk = `OXICLOUD_MAX_UPLOAD_SIZE × concurrent_PUTs`.
- **Chunked upload**: each in-flight session accumulates chunks under
  `OXICLOUD_CHUNK_DIR`, then assembles them into a single temp file
  before promotion. Worst case disk per session = **2 × file_size**
  (chunks + assembled file); total disk =
  `2 × OXICLOUD_MAX_UPLOAD_SIZE × concurrent_sessions`.

### Sizing examples

A 4 GB tmpfs serving a small team (~5 concurrent uploads):

| Setting | Disk worst case | Safe on 4 GB tmpfs? |
|---|---|---|
| `MAX_UPLOAD=10 GB`, no `CHUNK_MAX` tweak | 50 GB direct, 100 GB chunked | ❌ no — single upload OOMs the tmpfs |
| `MAX_UPLOAD=500 MB`, `CHUNK_MAX=50 MB` | 2.5 GB direct, 5 GB chunked | ⚠ direct fits, chunked overflows |
| `MAX_UPLOAD=300 MB`, `CHUNK_MAX=30 MB` | 1.5 GB direct, 3 GB chunked | ✅ both fit |

A real-disk volume (cheap, large):

| Setting | Disk worst case | Comment |
|---|---|---|
| `MAX_UPLOAD=10 GB`, `CHUNK_MAX=100 MB` (defaults) | 100 GB chunked worst case | Fine on a 200+ GB volume; almost any real-disk setup |
| `MAX_UPLOAD=100 GB`, `CHUNK_MAX=500 MB` | 1 TB chunked worst case | Plausible for video archives; needs a dedicated upload volume |

### Choosing tmpfs vs real disk

| Constraint | Choice |
|---|---|
| Upload caps × concurrent users ≤ free RAM × 0.5 | tmpfs OK (fast, atomic with `.blobs/` if also tmpfs) |
| Upload caps × concurrent users > free RAM × 0.5 | **real disk** — same FS as `.blobs/` ideal |
| Container with cgroup memory limit | **real disk** — tmpfs spool counts against the cgroup limit and triggers OOMKill |
| Multi-GB uploads expected | **real disk** — even small concurrency on tmpfs runs out of space |
| Small-file workload only (≤ 50 MB), high concurrency | tmpfs gives a noticeable intake speedup |

The defaults (`MAX_UPLOAD=10 GB`, `CHUNK_MAX=100 MB`) assume **real
disk**. Don't run the defaults against tmpfs unless you've sized it
for the worst case.

## TL;DR

| Variable | Default | Purpose |
|---|---|---|
| `OXICLOUD_STORAGE_PATH` | `./storage` | Where `.blobs/` lives (the canonical content store) |
| `OXICLOUD_UPLOAD_TMPDIR` | OS temp dir | Where non-chunked PUT bodies are spooled |
| `OXICLOUD_CHUNK_DIR` | `{STORAGE_PATH}/.uploads` | Where chunked-upload sessions accumulate |

The two rules that matter most:

1. **Put all three on the same filesystem.** Blob promotion is an
   atomic `rename(2)` when source and destination share an FS — cheap
   and crash-safe. Across filesystems it becomes a full `read + write +
   unlink`, multiplying the IO and widening the durability window.
2. **Don't leave the spool dir on tmpfs** (the default in many
   containers). Spool bodies count against the cgroup memory limit
   and can trigger OOMKill on multi-GB uploads.

## Where each upload surface spools

OxiCloud has several entry points that accept request bodies. They
land in different places by default:

| Surface | Default destination | Configurable via |
|---|---|---|
| REST chunked PUT (`PATCH /api/uploads/{id}`) | `{STORAGE_PATH}/.uploads/{upload_id}/chunk_NNNNNN` | `OXICLOUD_CHUNK_DIR` |
| REST chunked assemble (during `/complete`) | `{STORAGE_PATH}/.uploads/{upload_id}/assembled` | `OXICLOUD_CHUNK_DIR` |
| NextCloud chunked PUT (`PUT /dav/uploads/.../chunk`) | `{STORAGE_PATH}/.uploads/nextcloud/{user}/{upload_id}/{chunk_name}` | `OXICLOUD_CHUNK_DIR` |
| NextCloud chunked assemble (during `MOVE`) | `{STORAGE_PATH}/.uploads/nextcloud/{user}/{upload_id}/.assembled` | `OXICLOUD_CHUNK_DIR` |
| Native WebDAV PUT (`PUT /webdav/{path}`) | OS temp dir (`/tmp`) | `OXICLOUD_UPLOAD_TMPDIR` |
| NextCloud single-file PUT (`PUT /dav/files/.../{path}`) | OS temp dir | `OXICLOUD_UPLOAD_TMPDIR` |
| REST multipart upload (`POST /api/files/upload`) | `{STORAGE_PATH}/.dedup_temp/upload-{uuid}` | `OXICLOUD_STORAGE_PATH` (subdir is hard-wired) |
| Final blob storage (after fsync + rename) | `{STORAGE_PATH}/.blobs/{ab}/{abc…}.blob` | `OXICLOUD_STORAGE_PATH` |

## Why placement matters

### 1. Same filesystem ⇒ promotion is a rename

OxiCloud uses **content-addressable storage**: the final blob path is
derived from the file's BLAKE3 hash, which can only be known after the
last byte arrives. So every upload writes to a temp location first,
then **promotes** the temp file to `.blobs/{ab}/{abc…}.blob` by way of
a `rename(2)` call.

- **Same FS:** `rename` is atomic, O(1), no data copy. Total upload
  cost = body bytes received + one rename syscall. Crash-safe — the
  blob either exists at the final path or doesn't.
- **Cross-FS:** the kernel can't `rename(2)` across filesystems. The
  blob backend falls back to `fs::copy + fs::remove_file` (visible in
  `local_blob_backend.rs` as the EXDEV handler). Total cost = body
  bytes received + one full file copy. Doubles the IO bandwidth used
  per upload and widens the durability window.

### 2. Spool off tmpfs

`tempfile::NamedTempFile::new()` (used when `OXICLOUD_UPLOAD_TMPDIR`
is unset) honors `$TMPDIR`, which in many container setups points at
**tmpfs** — RAM-backed storage. A 2 GB upload spool then consumes 2 GB
of memory until the rename promotes it to disk.

In a Kubernetes pod with a 4 GB memory limit, the OOMKiller wakes up
long before the upload finishes. With `OXICLOUD_UPLOAD_TMPDIR` pointed
at a real-disk directory, the spool's memory footprint stays at ~one
HTTP frame regardless of file size.

### 3. NVMe for the hot path

The chunked-upload session directory sees a LOT of small writes —
each chunk PUT writes a file, the progress bitmap is rewritten after
each PUT, the assemble step reads them all back in order. Pointing
`OXICLOUD_CHUNK_DIR` at an NVMe device is a substantial win on
deployments that handle large file uploads, even if the final blob
storage is on slower disk.

The same applies to `OXICLOUD_UPLOAD_TMPDIR` (single-file PUTs).

A common high-throughput layout:

- **NVMe** (small, fast): `OXICLOUD_CHUNK_DIR`, `OXICLOUD_UPLOAD_TMPDIR`
- **HDD or NAS** (large, cheap): `OXICLOUD_STORAGE_PATH`/`.blobs/`

Trade-off: the rename optimization (rule 1) DOESN'T apply across
filesystems. If you split the hot path off the blob filesystem, every
upload pays a full file copy on promotion. You have to choose
between **fast intake** and **zero-copy promotion**.

| Goal | Layout | Cost per upload |
|---|---|---|
| Fastest possible intake | NVMe chunk dir + HDD blobs | 1× write to NVMe + 1× read NVMe + 1× write to HDD (copy) |
| Lowest IO + crash safety | NVMe everything OR HDD everything | 1× write to disk + 1 rename (~0 cost) |
| Default (do nothing) | Everything under `STORAGE_PATH` on whatever FS that is | Depends on `STORAGE_PATH` placement |

For most deployments **"same FS everywhere"** wins. The NVMe-split is
useful when intake latency dominates the user experience and you can
afford the doubled IO.

## Recommended layouts

### Single-disk box (most common)

Defaults are fine. Optionally set `OXICLOUD_UPLOAD_TMPDIR` to keep
the PUT spool off `/tmp`:

```bash
OXICLOUD_STORAGE_PATH=/var/lib/oxicloud
OXICLOUD_UPLOAD_TMPDIR=/var/lib/oxicloud/.spool
# OXICLOUD_CHUNK_DIR unset → /var/lib/oxicloud/.uploads
```

All three on the same filesystem → rename promotion → atomic and fast.

### Container with constrained memory

Critical: make sure neither spool sits on tmpfs.

```bash
OXICLOUD_STORAGE_PATH=/data
OXICLOUD_UPLOAD_TMPDIR=/data/.spool
OXICLOUD_CHUNK_DIR=/data/.uploads
```

If you can't mount a writable `/data`, at minimum bind-mount a real
volume at the spool dirs.

### Split-disk (NVMe intake + HDD blobs)

```bash
OXICLOUD_STORAGE_PATH=/mnt/hdd/oxicloud      # .blobs/ + .dedup_temp/
OXICLOUD_UPLOAD_TMPDIR=/mnt/nvme/oxi-spool
OXICLOUD_CHUNK_DIR=/mnt/nvme/oxi-chunks
```

Faster intake; pays a copy on promotion. Worth it when uploads are
many small files (NVMe IOPS dominates) or when intake latency directly
hits user-visible UX.

## Sharing the spool and chunk directories

Pointing `OXICLOUD_UPLOAD_TMPDIR` and `OXICLOUD_CHUNK_DIR` at the
**same directory** is supported by design. Each writer tags its
output so the surfaces never interfere with each other:

| Writer | On-disk name pattern |
|---|---|
| PUT spool (single-file uploads) | `.tmpXXXXXXXX` — files (not directories), random suffix |
| REST chunked sessions | `oxi-chunk-{uuid}/` — directories with a well-known prefix |
| NC chunked subtree | `nextcloud/{user}/{uuid}/` — under its own root subdir |

The 24-hour orphan-session cleanup loop filters strictly on the
`oxi-chunk-` prefix, so it can NEVER delete a non-OxiCloud directory
that happens to live alongside chunked sessions. The PUT spool's
`.tmpXXXX` files are files (not directories) and the NC subtree's
`nextcloud/` root has its own name — both are invisible to the
cleanup loop.

**Recommendation:** for new deployments, use separate directories
anyway (the defaults `.spool/` and `.uploads/` already do this) —
it makes disk-usage attribution clearer and keeps IOPS isolated when
both are busy. Shared directories are safe to use when disk layout
forces it.

## What's NOT yet configurable

- **REST multipart upload directory** (`POST /api/files/upload`) is
  hard-wired to `{STORAGE_PATH}/.dedup_temp/`. It can't be moved
  separately. Same-FS placement is automatic.
- **WOPI PutFile spool** (Office editor saves) uses the bare OS temp
  dir without honoring `OXICLOUD_UPLOAD_TMPDIR`. This is a known
  inconsistency and on the hardening backlog.
- **Per-user / per-drive spool directories** — all users share the
  same `OXICLOUD_CHUNK_DIR` root today. Multi-tenant isolation
  through separate spool dirs isn't supported.

## Quick verification

Boot the server with `RUST_LOG=info` and the first lines after the
banner include:

```
oxicloud: Upload limits loaded from config max_upload_size_mb=10240 chunk_max_bytes_mb=100
```

That confirms the upload-cap env vars were read. To confirm
directory placement, watch for chunk file creation under your
`OXICLOUD_CHUNK_DIR` (or its default `{STORAGE_PATH}/.uploads/`)
during a chunked upload — `ls` while a sync is in progress shows the
`{uuid}/chunk_NNNNNN` files appearing in real time.
