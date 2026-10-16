//! Rectangular capture regions in monitor-relative coordinates.

use std::fmt;
use std::str::FromStr;

/// A rectangle relative to a monitor's top-left corner, in the same units
/// as `Monitor::width()` / `Monitor::height()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Builds a region from a mouse drag in any direction.
    /// Points left of / above the monitor are clamped to 0.
    pub fn from_drag(start: (i32, i32), end: (i32, i32)) -> Self {
        let x0 = start.0.min(end.0).max(0) as u32;
        let y0 = start.1.min(end.1).max(0) as u32;
        let x1 = start.0.max(end.0).max(0) as u32;
        let y1 = start.1.max(end.1).max(0) as u32;
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Clips the region to `max_width` x `max_height`.
    /// Returns `None` if nothing is left.
    pub fn clamp_to(self, max_width: u32, max_height: u32) -> Option<Self> {
        if self.x >= max_width || self.y >= max_height {
            return None;
        }
        let clipped = Self::new(
            self.x,
            self.y,
            self.width.min(max_width - self.x),
            self.height.min(max_height - self.y),
        );
        (!clipped.is_empty()).then_some(clipped)
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{},{},{}", self.x, self.y, self.width, self.height)
    }
}

/// Parses `x,y,width,height`, e.g. `100,200,640,480`.
impl FromStr for Region {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts = s
            .split(',')
            .map(|p| p.trim().parse::<u32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("invalid region '{s}': {e}"))?;
        match parts[..] {
            [x, y, w, h] => Ok(Self::new(x, y, w, h)),
            _ => Err(format!("invalid region '{s}': expected x,y,width,height")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_top_left_to_bottom_right() {
        assert_eq!(
            Region::from_drag((10, 20), (110, 220)),
            Region::new(10, 20, 100, 200)
        );
    }

    #[test]
    fn drag_bottom_right_to_top_left() {
        assert_eq!(
            Region::from_drag((110, 220), (10, 20)),
            Region::new(10, 20, 100, 200)
        );
    }

    #[test]
    fn drag_outside_monitor_clamps_to_zero() {
        assert_eq!(
            Region::from_drag((-50, -50), (100, 100)),
            Region::new(0, 0, 100, 100)
        );
    }

    #[test]
    fn click_without_drag_is_empty() {
        assert!(Region::from_drag((50, 50), (50, 50)).is_empty());
    }

    #[test]
    fn clamp_inside_is_unchanged() {
        let r = Region::new(10, 10, 100, 100);
        assert_eq!(r.clamp_to(1920, 1080), Some(r));
    }

    #[test]
    fn clamp_clips_overflow() {
        assert_eq!(
            Region::new(1900, 1000, 100, 100).clamp_to(1920, 1080),
            Some(Region::new(1900, 1000, 20, 80))
        );
    }

    #[test]
    fn clamp_outside_is_none() {
        assert_eq!(Region::new(1920, 0, 10, 10).clamp_to(1920, 1080), None);
        assert_eq!(Region::new(0, 1080, 10, 10).clamp_to(1920, 1080), None);
    }

    #[test]
    fn clamp_empty_is_none() {
        assert_eq!(Region::new(10, 10, 0, 50).clamp_to(1920, 1080), None);
    }

    #[test]
    fn parse_and_display_roundtrip() {
        let r: Region = "100, 200,640,480".parse().unwrap();
        assert_eq!(r, Region::new(100, 200, 640, 480));
        assert_eq!(r.to_string(), "100,200,640,480");
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!("1,2,3".parse::<Region>().is_err());
        assert!("1,2,3,4,5".parse::<Region>().is_err());
        assert!("a,b,c,d".parse::<Region>().is_err());
        assert!("-1,0,10,10".parse::<Region>().is_err());
    }
}
