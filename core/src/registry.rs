//! The mark-capability registry: the single source of truth for what each mark supports.
//!
//! Both the backend (how to draw a mark) and the panel (which controls to show) read
//! `caps(mark)` so SPEC §2.2 — "the format is bounded by what is drawable" — holds by
//! construction rather than by parallel hand-wiring. Adding a future mark is a single new
//! arm here plus one backend draw routine; the UI adapts automatically (IMPLEMENTATION §17).

use crate::dsl::Mark;

/// Which encoding channels a mark accepts. A `true` field means binding that channel is
/// meaningful for the mark; the resolver warns when a bound channel is `false` here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelSet {
    pub x: bool,
    pub y: bool,
    pub color: bool,
    pub size: bool,
    pub theta: bool,
}

/// Which presentation/transform options apply to a mark. The panel shows only the option
/// widgets whose flag is `true`; the backend reads the same flags when drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionSet {
    pub stack: bool,
    pub interpolate: bool,
    pub orientation: bool,
    pub bins: bool,
    pub inner_radius: bool,
    pub point_size: bool,
    pub line_width: bool,
    pub fill_opacity: bool,
    /// Axis scales (log/sqrt/domain/zero) apply — cartesian marks only.
    pub scales: bool,
}

/// A mark's full capability profile: whether it is cartesian (vs radial), the channels it
/// accepts, and the options it honors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkCaps {
    /// `true` for x/y plotted marks; `false` for radial marks (Arc) drawn without axes.
    pub cartesian: bool,
    pub channels: ChannelSet,
    pub options: OptionSet,
}

/// All channels off — a base to flip the supported ones on from, keeping `caps` arms terse.
const NO_CHANNELS: ChannelSet = ChannelSet {
    x: false,
    y: false,
    color: false,
    size: false,
    theta: false,
};

/// All options off — a base for each `caps` arm to enable only what the mark supports.
const NO_OPTIONS: OptionSet = OptionSet {
    stack: false,
    interpolate: false,
    orientation: false,
    bins: false,
    inner_radius: false,
    point_size: false,
    line_width: false,
    fill_opacity: false,
    scales: false,
};

/// The capability profile for `mark` — the single source of truth read by the backend and
/// the panel. An exhaustive match, one arm per mark, so adding a `Mark` variant forces a
/// decision here (IMPLEMENTATION §17.1).
pub const fn caps(mark: Mark) -> MarkCaps {
    match mark {
        Mark::Bar => MarkCaps {
            cartesian: true,
            channels: ChannelSet { x: true, y: true, color: true, ..NO_CHANNELS },
            options: OptionSet {
                stack: true,
                orientation: true,
                fill_opacity: true,
                scales: true,
                ..NO_OPTIONS
            },
        },
        Mark::Line => MarkCaps {
            cartesian: true,
            channels: ChannelSet { x: true, y: true, color: true, ..NO_CHANNELS },
            options: OptionSet {
                interpolate: true,
                line_width: true,
                scales: true,
                ..NO_OPTIONS
            },
        },
        Mark::Point => MarkCaps {
            cartesian: true,
            channels: ChannelSet { x: true, y: true, color: true, size: true, ..NO_CHANNELS },
            options: OptionSet { point_size: true, scales: true, ..NO_OPTIONS },
        },
        Mark::Area => MarkCaps {
            cartesian: true,
            channels: ChannelSet { x: true, y: true, color: true, ..NO_CHANNELS },
            options: OptionSet {
                stack: true,
                line_width: true,
                fill_opacity: true,
                scales: true,
                ..NO_OPTIONS
            },
        },
        Mark::Histogram => MarkCaps {
            cartesian: true,
            channels: ChannelSet { x: true, color: true, ..NO_CHANNELS },
            options: OptionSet { bins: true, fill_opacity: true, scales: true, ..NO_OPTIONS },
        },
        Mark::Arc => MarkCaps {
            cartesian: false,
            channels: ChannelSet { color: true, theta: true, ..NO_CHANNELS },
            options: OptionSet { inner_radius: true, fill_opacity: true, ..NO_OPTIONS },
        },
        // The table mark uses no encoding channels and no plotted options: which columns it
        // shows (and the transpose toggle) are mark-specific controls the panel handles
        // directly, not channels/options gated through the registry.
        Mark::Table => MarkCaps {
            cartesian: false,
            channels: NO_CHANNELS,
            options: NO_OPTIONS,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::caps;
    use crate::dsl::Mark;

    #[test]
    fn bar_is_cartesian_with_xy_color_and_stack() {
        let c = caps(Mark::Bar);
        assert!(c.cartesian);
        assert!(c.channels.x && c.channels.y && c.channels.color);
        assert!(!c.channels.size && !c.channels.theta);
        assert!(c.options.stack && c.options.orientation && c.options.scales);
        assert!(!c.options.bins && !c.options.inner_radius);
    }

    #[test]
    fn line_supports_interpolate_not_stack() {
        let c = caps(Mark::Line);
        assert!(c.options.interpolate && c.options.line_width);
        assert!(!c.options.stack && !c.options.orientation);
    }

    #[test]
    fn point_supports_size_channel() {
        let c = caps(Mark::Point);
        assert!(c.channels.size);
        assert!(c.options.point_size && c.options.scales);
    }

    #[test]
    fn area_stacks_and_fills() {
        let c = caps(Mark::Area);
        assert!(c.options.stack && c.options.fill_opacity && c.options.line_width);
    }

    #[test]
    fn histogram_takes_x_and_bins_not_y_channel() {
        let c = caps(Mark::Histogram);
        assert!(c.cartesian);
        assert!(c.channels.x && c.channels.color);
        assert!(!c.channels.y, "y is the computed frequency, not a bound channel");
        assert!(c.options.bins && c.options.scales);
    }

    #[test]
    fn arc_is_radial_with_theta_and_inner_radius() {
        let c = caps(Mark::Arc);
        assert!(!c.cartesian, "arc is radial, no cartesian axes");
        assert!(c.channels.theta && c.channels.color);
        assert!(!c.channels.x && !c.channels.y);
        assert!(c.options.inner_radius && c.options.fill_opacity);
        assert!(!c.options.scales, "radial marks have no axis scales");
    }

    #[test]
    fn no_mark_enables_scales_without_being_cartesian() {
        for mark in [
            Mark::Bar,
            Mark::Line,
            Mark::Point,
            Mark::Area,
            Mark::Histogram,
            Mark::Arc,
            Mark::Table,
        ] {
            let c = caps(mark);
            assert!(!c.options.scales || c.cartesian, "scales imply cartesian for {mark:?}");
        }
    }

    #[test]
    fn table_is_noncartesian_with_no_channels_or_options() {
        let c = caps(Mark::Table);
        assert!(!c.cartesian);
        assert!(!c.channels.x && !c.channels.y && !c.channels.color);
        assert!(!c.channels.size && !c.channels.theta);
        assert!(!c.options.stack && !c.options.scales && !c.options.fill_opacity);
    }
}
