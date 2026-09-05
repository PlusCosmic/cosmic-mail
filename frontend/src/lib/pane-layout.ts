export const PANE_RATIO_STORAGE_KEY = "cosmic-mail:list-pane-ratio";
export const DEFAULT_LIST_PANE_RATIO = 0.42;
export const MIN_LIST_PANE_WIDTH = 320;
export const MAX_LIST_PANE_WIDTH = 760;
export const MIN_READER_PANE_WIDTH = 320;
export const PANE_DIVIDER_WIDTH = 9;

export interface PaneWidthLimits {
	minimum: number;
	maximum: number;
}

function clamp(value: number, minimum: number, maximum: number): number {
	return Math.min(Math.max(value, minimum), maximum);
}

/**
 * Return list-pane limits for the width shared by the list and reader. At very
 * small widths the list minimum yields first, so the reader never disappears.
 */
export function paneWidthLimits(availableWidth: number): PaneWidthLimits {
	const width = Math.max(0, availableWidth);
	const minimum = Math.min(MIN_LIST_PANE_WIDTH, Math.max(0, width - MIN_READER_PANE_WIDTH));
	const maximum = Math.max(
		minimum,
		Math.min(MAX_LIST_PANE_WIDTH, Math.max(0, width - MIN_READER_PANE_WIDTH)),
	);

	return { minimum, maximum };
}

export function resolveListPaneWidth(availableWidth: number, preferredRatio: number): number {
	const limits = paneWidthLimits(availableWidth);
	const ratio = isValidPaneRatio(preferredRatio)
		? preferredRatio
		: DEFAULT_LIST_PANE_RATIO;
	return clamp(availableWidth * ratio, limits.minimum, limits.maximum);
}

export function ratioForListPaneWidth(availableWidth: number, listWidth: number): number {
	if (!Number.isFinite(availableWidth) || availableWidth <= 0) {
		return DEFAULT_LIST_PANE_RATIO;
	}
	return clamp(listWidth / availableWidth, 0, 1);
}

export function isValidPaneRatio(value: unknown): value is number {
	return typeof value === "number" && Number.isFinite(value) && value > 0 && value < 1;
}
