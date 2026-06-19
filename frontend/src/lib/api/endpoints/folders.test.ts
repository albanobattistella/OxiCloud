import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('$lib/api/client', () => ({ apiFetch: vi.fn(), apiJson: vi.fn() }));

import { apiFetch, apiJson } from '$lib/api/client';
import type { FolderItem } from '$lib/api/types';
import {
	fetchFolderListing,
	getCachedFolder,
	cacheFolder,
	invalidateFolderCache,
	getFolder,
	getFolderName,
	rememberFolderName,
	type FolderListing
} from './folders';

type RawListing = {
	folders?: unknown[];
	files?: unknown[];
	favorite_ids?: string[];
	shared_ids?: string[];
};

function fakeRes(opts: { status: number; body?: RawListing; etag?: string }): Response {
	return {
		status: opts.status,
		ok: opts.status >= 200 && opts.status < 300,
		json: async () => opts.body ?? {},
		headers: { get: (k: string) => (k.toLowerCase() === 'etag' ? (opts.etag ?? null) : null) }
	} as unknown as Response;
}

const emptyListing = (): FolderListing => ({
	folders: [],
	files: [],
	favoriteIds: [],
	sharedIds: []
});

const initHeaders = (call: number): Record<string, string> =>
	(vi.mocked(apiFetch).mock.calls[call][1]?.headers ?? {}) as Record<string, string>;

beforeEach(() => {
	vi.clearAllMocks();
	invalidateFolderCache();
});

describe('fetchFolderListing (conditional)', () => {
	it('parses a 200, returns the ETag, and sends no If-None-Match without one', async () => {
		vi.mocked(apiFetch).mockResolvedValue(
			fakeRes({
				status: 200,
				body: { folders: [], files: [], favorite_ids: ['a'], shared_ids: ['b'] },
				etag: '"v1"'
			})
		);
		const r = await fetchFolderListing('f1');
		expect(r.status).toBe(200);
		expect(r.etag).toBe('"v1"');
		expect(r.listing?.favoriteIds).toEqual(['a']);
		expect(r.listing?.sharedIds).toEqual(['b']);
		expect(initHeaders(0)['If-None-Match']).toBeUndefined();
		// No cache-busting query param — the URL must be stable for revalidation.
		expect(vi.mocked(apiFetch).mock.calls[0][0]).toBe('/api/folders/f1/listing');
	});

	it('sends If-None-Match and surfaces a 304 with no body', async () => {
		vi.mocked(apiFetch).mockResolvedValue(fakeRes({ status: 304 }));
		const r = await fetchFolderListing('f1', { etag: '"v1"' });
		expect(r.status).toBe(304);
		expect(r.listing).toBeUndefined();
		expect(initHeaders(0)['If-None-Match']).toBe('"v1"');
	});

	it('throws a 403 carrying its status', async () => {
		vi.mocked(apiFetch).mockResolvedValue(fakeRes({ status: 403 }));
		await expect(fetchFolderListing('f1')).rejects.toMatchObject({ status: 403 });
	});
});

describe('folder listing cache (LRU + invalidation)', () => {
	it('stores and retrieves a listing + its ETag', () => {
		cacheFolder('a', emptyListing(), '"1"');
		expect(getCachedFolder('a')?.etag).toBe('"1"');
		expect(getCachedFolder('missing')).toBeUndefined();
	});

	it('evicts the least-recently-used entry past the cap', () => {
		for (let i = 0; i < 45; i++) cacheFolder(`f${i}`, emptyListing());
		expect(getCachedFolder('f0')).toBeUndefined(); // evicted (cap is 40)
		expect(getCachedFolder('f44')).toBeDefined();
	});

	it('a read bumps recency so the touched entry survives eviction', () => {
		for (let i = 0; i < 40; i++) cacheFolder(`f${i}`, emptyListing());
		getCachedFolder('f0'); // bump f0 to most-recent
		cacheFolder('extra', emptyListing()); // forces one eviction
		expect(getCachedFolder('f0')).toBeDefined();
		expect(getCachedFolder('f1')).toBeUndefined(); // f1 was now the oldest
	});

	it('invalidates a single folder, or the whole cache', () => {
		cacheFolder('a', emptyListing());
		cacheFolder('b', emptyListing());
		invalidateFolderCache('a');
		expect(getCachedFolder('a')).toBeUndefined();
		expect(getCachedFolder('b')).toBeDefined();
		invalidateFolderCache();
		expect(getCachedFolder('b')).toBeUndefined();
	});
});

describe('folder name cache (breadcrumbs)', () => {
	const folder = (id: string, name: string): FolderItem => ({ id, name }) as unknown as FolderItem;

	it("learns its children's names from a cached listing", () => {
		cacheFolder('nc-parent', {
			folders: [folder('nc-a', 'Alpha'), folder('nc-b', 'Beta')],
			files: [],
			favoriteIds: [],
			sharedIds: []
		});
		expect(getFolderName('nc-a')).toBe('Alpha');
		expect(getFolderName('nc-b')).toBe('Beta');
		expect(getFolderName('nc-unknown')).toBeUndefined();
	});

	it('records the name fetched by getFolder', async () => {
		vi.mocked(apiJson).mockResolvedValue(folder('gf-1', 'Reports') as never);
		const f = await getFolder('gf-1');
		expect(f.name).toBe('Reports');
		expect(getFolderName('gf-1')).toBe('Reports');
	});

	it('rememberFolderName overwrites a stale name (e.g. after a rename)', () => {
		rememberFolderName('rn-1', 'Old');
		expect(getFolderName('rn-1')).toBe('Old');
		rememberFolderName('rn-1', 'New');
		expect(getFolderName('rn-1')).toBe('New');
	});
});
