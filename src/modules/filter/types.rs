//! The filter's responses.

/// Filter type selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterType {
    /// Low-pass filter - removes frequencies above cutoff.
    #[default]
    LowPass,
    /// High-pass filter - removes frequencies below cutoff.
    HighPass,
    /// Band-pass filter - passes frequencies around cutoff.
    BandPass,
    /// Bell - boosts or cuts around the cutoff.
    Bell,
    /// Low shelf - boosts or cuts below the cutoff.
    LowShelf,
    /// High shelf - boosts or cuts above the cutoff.
    HighShelf,
    /// Notch - removes the cutoff.
    Notch,
}

impl FilterType {
    const ALL: [FilterType; 7] = [
        FilterType::LowPass,
        FilterType::HighPass,
        FilterType::BandPass,
        FilterType::Bell,
        FilterType::LowShelf,
        FilterType::HighShelf,
        FilterType::Notch,
    ];

    /// Its position among the `filter_type` options.
    pub(super) fn position(self) -> u32 {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0) as u32
    }

    /// The type at `position` among the `filter_type` options.
    pub(super) fn from_position(position: u32) -> Option<Self> {
        Self::ALL.get(position as usize).copied()
    }

    /// Whether this is an EQ response (shaped by `boost` and `q`).
    pub(super) fn is_eq(self) -> bool {
        !matches!(
            self,
            FilterType::LowPass | FilterType::HighPass | FilterType::BandPass
        )
    }
}
