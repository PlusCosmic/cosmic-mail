//! List/reader split geometry (pure).

pub const DEFAULT_LIST_PANE_RATIO: f32 = 0.42;
pub const MIN_LIST_PANE_WIDTH: f32 = 320.0;
pub const MAX_LIST_PANE_WIDTH: f32 = 760.0;
pub const MIN_READER_PANE_WIDTH: f32 = 320.0;
pub const PANE_DIVIDER_WIDTH: f32 = 9.0;

/// The list pane's allowed width range for the width shared by list and reader.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneWidthLimits {
    pub minimum: f32,
    pub maximum: f32,
}

/// List-pane limits. At very small widths the list minimum yields first, so
/// the reader never disappears.
pub fn pane_width_limits(available: f32) -> PaneWidthLimits {
    let width = available.max(0.0);
    let room = (width - MIN_READER_PANE_WIDTH).max(0.0);
    let minimum = MIN_LIST_PANE_WIDTH.min(room);
    let maximum = minimum.max(MAX_LIST_PANE_WIDTH.min(room));
    PaneWidthLimits { minimum, maximum }
}

/// The list width for a preferred ratio, clamped to the limits.
pub fn resolve_list_pane_width(available: f32, preferred_ratio: f32) -> f32 {
    let limits = pane_width_limits(available);
    let ratio = if is_valid_pane_ratio(preferred_ratio) {
        preferred_ratio
    } else {
        DEFAULT_LIST_PANE_RATIO
    };
    (available * ratio).clamp(limits.minimum, limits.maximum)
}

/// The ratio a dragged list width represents.
pub fn ratio_for_list_pane_width(available: f32, list_width: f32) -> f32 {
    if !available.is_finite() || available <= 0.0 {
        return DEFAULT_LIST_PANE_RATIO;
    }
    (list_width / available).clamp(0.0, 1.0)
}

/// Whether a persisted ratio is usable.
pub fn is_valid_pane_ratio(value: f32) -> bool {
    value.is_finite() && value > 0.0 && value < 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_preferred_ratio_when_both_panes_remain_usable() {
        assert_eq!(resolve_list_pane_width(1000.0, 0.42), 420.0);
    }

    #[test]
    fn clamps_list_width_at_both_bounds() {
        assert_eq!(resolve_list_pane_width(1000.0, 0.1), MIN_LIST_PANE_WIDTH);
        assert_eq!(resolve_list_pane_width(3000.0, 0.9), MAX_LIST_PANE_WIDTH);
    }

    #[test]
    fn preserves_reader_space_when_the_available_width_is_small() {
        assert_eq!(
            pane_width_limits(500.0),
            PaneWidthLimits {
                minimum: 180.0,
                maximum: 180.0
            }
        );
        assert_eq!(
            resolve_list_pane_width(500.0, DEFAULT_LIST_PANE_RATIO),
            180.0
        );
    }

    #[test]
    fn converts_an_adjusted_width_back_into_a_ratio() {
        assert_eq!(ratio_for_list_pane_width(1200.0, 480.0), 0.4);
        assert_eq!(
            ratio_for_list_pane_width(0.0, 480.0),
            DEFAULT_LIST_PANE_RATIO
        );
    }

    #[test]
    fn rejects_malformed_persisted_ratios() {
        assert!(is_valid_pane_ratio(0.42));
        assert!(!is_valid_pane_ratio(f32::NAN));
        assert!(!is_valid_pane_ratio(0.0));
        assert!(!is_valid_pane_ratio(1.0));
    }
}
