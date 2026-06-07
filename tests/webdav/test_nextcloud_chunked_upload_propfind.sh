#!/usr/bin/env bash
# =============================================================
# OxiCloud – NextCloud chunked-upload PROPFIND (resume support)
# =============================================================
# Verifies the PROPFIND handler on /remote.php/dav/uploads/{user}/{upload_id}.
#
# Why this matters: the NextCloud Android client (and several other
# mobile NC clients) issue PROPFIND on the upload-session URL before
# resuming an interrupted chunked upload. Without it the client gets
# 405 METHOD_NOT_ALLOWED and either restarts the whole upload from
# scratch or fails outright. This was a real bug reported against the
# OxiCloud NC gateway by an admin running NC Android 3.31.1.
#
# Sequence:
#   1. Login → JWT, then mint an app password (NC surface requires
#      Basic Auth with an app password, not the user's login password).
#   2. MKCOL the session → 201.
#   3. PROPFIND empty session → 207 Multi-Status with exactly ONE
#      <d:response> entry (the session collection itself).
#   4. PUT two chunks (00000001, 00000002).
#   5. PROPFIND again → 207 with THREE entries (collection + 2 chunks).
#      Asserts the chunk byte counts come back correctly so the
#      Android client's resume logic can compare against its expected
#      chunk sizes.
#   6. Security checks:
#        a. PROPFIND with mismatched URL user → 403.
#        b. PROPFIND on a session that doesn't exist → 404.
#        c. PROPFIND with no auth → 401.
#   7. DELETE the session → 204.
#
# Prerequisites:
#   - Server running at base_url
#   - OXICLOUD_ENABLE_AUTH=true (NC surface is auth-only)
#   - jq, curl, xmllint in PATH
#
# Run (from repo root):
#   bash tests/webdav/test_nextcloud_chunked_upload_propfind.sh
# =============================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

source test.env
source common.sh

# ── helpers ──────────────────────────────────────────────────────────────────

PASS=0
FAIL=0

pass() { PASS=$(( PASS + 1 )); echo "  PASS: $*"; }
fail() { FAIL=$(( FAIL + 1 )); echo "  FAIL: $*" >&2; exit 1; }

# All NC-surface curls go through this helper so the Basic-Auth
# header (app password, not the user's login password) is applied
# uniformly. The NC handler rejects login-password Basic Auth — only
# app passwords pass the verify_basic_auth check.
nc_curl() {
    curl -s -u "$username:$APP_PASS" "$@"
}

# Mint an app password using the JWT we just got. The NC handlers
# accept only app-password Basic Auth (rfc 7617), never the login
# password, so this step is mandatory.
#
# Request DTO is `CreateAppPasswordRequestDto`: `label` is required,
# `scopes` and `expires_in_days` are optional. Sending the wrong
# field name makes axum's Json extractor return a plain-text error
# body that subsequent jq parsing chokes on with "Invalid numeric
# literal" — a confusing error mode worth pinning the field name
# against.
mint_app_password() {
    local response
    response=$(curl -s -X POST \
        -H "Authorization: Bearer $TOKEN" \
        -H "Content-Type: application/json" \
        -d '{"label":"nc-chunked-upload-propfind-test"}' \
        "$base_url/api/auth/app-passwords")
    APP_PASS=$(jq -r '.password // empty' <<< "$response" 2>/dev/null || echo "")
    [[ -n "$APP_PASS" ]] || fail "Could not mint app password (response: $response)"
}

# Count <d:response> children in a multistatus body. We don't use a
# full XML parser because the response shape is fixed by our handler
# and a plain grep is robust enough for the assertions we need.
count_responses() {
    grep -o '<d:response>' <<< "$1" | wc -l | tr -d ' '
}

echo
echo "=== NextCloud chunked-upload PROPFIND (resume support) ==="
echo

# ── authenticate + mint app password ─────────────────────────────────────────

oxicloud_login
mint_app_password

UPLOAD_ID="test-propfind-$(date +%s)-$$"
NC_BASE="$base_url/remote.php/dav/uploads/$username/$UPLOAD_ID"

# Idempotent pre-test cleanup — drop any stale session from a previous run.
nc_curl -o /dev/null -X DELETE "$NC_BASE" > /dev/null 2>&1 || true

# ── Step 1: MKCOL — create session ───────────────────────────────────────────

echo "  step 1: MKCOL $NC_BASE"
STATUS=$(nc_curl -o /dev/null -w "%{http_code}" -X MKCOL "$NC_BASE")
[[ "$STATUS" == "201" ]] || fail "MKCOL expected 201, got $STATUS"
pass "Session created (MKCOL → 201)"

# ── Step 2: PROPFIND on empty session — only the collection ──────────────────

echo "  step 2: PROPFIND on empty session"
BODY=$(nc_curl -X PROPFIND -H "Depth: 1" "$NC_BASE")
[[ "$BODY" == *"<d:multistatus"* ]] \
    || fail "PROPFIND empty session: not a multistatus body (got: $BODY)"
N=$(count_responses "$BODY")
[[ "$N" == "1" ]] \
    || fail "PROPFIND empty session: expected 1 <d:response>, got $N (body: $BODY)"
[[ "$BODY" == *"<d:collection/>"* ]] \
    || fail "PROPFIND empty session: collection marker missing"
pass "Empty-session PROPFIND returns 1 response (collection only)"

# ── Step 3: PUT two chunks ───────────────────────────────────────────────────

echo "  step 3: PUT chunks 00000001 and 00000002"
CHUNK1_SIZE=$(printf "first chunk bytes" | wc -c | tr -d ' ')
CHUNK2_SIZE=$(printf "second chunk has different size" | wc -c | tr -d ' ')

STATUS=$(printf "first chunk bytes" | nc_curl -o /dev/null -w "%{http_code}" \
    -X PUT -H "Content-Type: application/octet-stream" --data-binary @- \
    "$NC_BASE/00000001")
[[ "$STATUS" == "201" ]] || fail "PUT chunk 00000001 expected 201, got $STATUS"

STATUS=$(printf "second chunk has different size" | nc_curl -o /dev/null -w "%{http_code}" \
    -X PUT -H "Content-Type: application/octet-stream" --data-binary @- \
    "$NC_BASE/00000002")
[[ "$STATUS" == "201" ]] || fail "PUT chunk 00000002 expected 201, got $STATUS"
pass "Both chunks uploaded ($CHUNK1_SIZE B + $CHUNK2_SIZE B)"

# ── Step 4: PROPFIND populated session ───────────────────────────────────────

echo "  step 4: PROPFIND populated session"
BODY=$(nc_curl -X PROPFIND -H "Depth: 1" "$NC_BASE")
N=$(count_responses "$BODY")
[[ "$N" == "3" ]] \
    || fail "PROPFIND populated session: expected 3 <d:response>, got $N (body: $BODY)"
pass "Populated PROPFIND returns 3 responses (collection + 2 chunks)"

# Chunk hrefs appear in the body.
[[ "$BODY" == *"/$UPLOAD_ID/00000001"* ]] \
    || fail "PROPFIND populated session: chunk 00000001 href missing"
[[ "$BODY" == *"/$UPLOAD_ID/00000002"* ]] \
    || fail "PROPFIND populated session: chunk 00000002 href missing"
pass "Both chunk hrefs present in multistatus body"

# Chunk content-lengths match what we uploaded — this is the field
# the Android client reads to decide whether a stored chunk is
# "complete" or "partial".
[[ "$BODY" == *"<d:getcontentlength>$CHUNK1_SIZE</d:getcontentlength>"* ]] \
    || fail "PROPFIND: chunk 00000001 size $CHUNK1_SIZE missing in body"
[[ "$BODY" == *"<d:getcontentlength>$CHUNK2_SIZE</d:getcontentlength>"* ]] \
    || fail "PROPFIND: chunk 00000002 size $CHUNK2_SIZE missing in body"
pass "Chunk getcontentlength values match upload byte counts"

# ── Step 5a: Security — URL user doesn't match auth user → 403 ────────────────

echo "  step 5a: PROPFIND with mismatched URL user → 403"
STATUS=$(nc_curl -o /dev/null -w "%{http_code}" -X PROPFIND \
    "$base_url/remote.php/dav/uploads/someoneelse/$UPLOAD_ID")
[[ "$STATUS" == "403" ]] \
    || fail "Mismatched-user PROPFIND expected 403, got $STATUS"
pass "Mismatched URL user rejected with 403"

# ── Step 5b: Security — nonexistent session → 404 ────────────────────────────

echo "  step 5b: PROPFIND on nonexistent session → 404"
STATUS=$(nc_curl -o /dev/null -w "%{http_code}" -X PROPFIND \
    "$base_url/remote.php/dav/uploads/$username/does-not-exist-$$-$(date +%s)")
[[ "$STATUS" == "404" ]] \
    || fail "Nonexistent-session PROPFIND expected 404, got $STATUS"
pass "Nonexistent session returns 404"

# ── Step 5c: Security — no auth → 401 ────────────────────────────────────────

echo "  step 5c: PROPFIND with no auth → 401"
STATUS=$(curl -s -o /dev/null -w "%{http_code}" -X PROPFIND "$NC_BASE")
[[ "$STATUS" == "401" ]] \
    || fail "Unauthenticated PROPFIND expected 401, got $STATUS"
pass "Unauthenticated request rejected with 401"

# ── Cleanup ──────────────────────────────────────────────────────────────────

echo "  cleanup..."
STATUS=$(nc_curl -o /dev/null -w "%{http_code}" -X DELETE "$NC_BASE")
[[ "$STATUS" == "204" ]] || fail "DELETE session expected 204, got $STATUS"
pass "Session deleted"

# Confirm cleanup: PROPFIND now returns 404.
STATUS=$(nc_curl -o /dev/null -w "%{http_code}" -X PROPFIND "$NC_BASE")
[[ "$STATUS" == "404" ]] \
    || fail "Post-delete PROPFIND expected 404, got $STATUS"
pass "Post-delete PROPFIND confirms session gone"

# ── summary ───────────────────────────────────────────────────────────────────

echo
echo "Results: $PASS passed, $FAIL failed."
[[ "$FAIL" -eq 0 ]] && echo "All tests passed." || exit 1
