// Pure selection helpers — no Svelte/DOM/store dependencies, so they can be
// exercised directly from node:test (tests/selection.test.mjs).

/**
 * Choose which item to select after removing `removedId` from `items`.
 *
 * Prefers the item that followed the removed one (which slides into its place),
 * then falls back to the item that preceded it, and returns `null` when the
 * removed item was the only entry or is not present. `items` is the list as it
 * stands *before* removal.
 */
export function nextSelectionId<T extends { id: number }>(
	items: readonly T[],
	removedId: number,
): number | null {
	const i = items.findIndex((item) => item.id === removedId);
	if (i < 0) return null;
	if (i + 1 < items.length) return items[i + 1].id;
	if (i - 1 >= 0) return items[i - 1].id;
	return null;
}
