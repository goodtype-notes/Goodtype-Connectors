//! Page-point geometry shared by routing and shape building. 72 points equal one inch.
use serde::{Deserialize, Serialize};

/// Coordinates closer than this are treated as equal.
pub const EPSILON: f64 = 1e-6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn plus(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
    pub fn minus(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
    pub fn scale(self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor)
    }
    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }
    /// The unit vector, or `None` for a (near) zero vector.
    pub fn unit(self) -> Option<Self> {
        let length = self.length();
        (length > EPSILON).then(|| self.scale(1.0 / length))
    }
    pub fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
    pub fn near(self, other: Self) -> bool {
        (self.x - other.x).abs() <= EPSILON && (self.y - other.y).abs() <= EPSILON
    }
}

/// An axis-aligned rectangle in page points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn right(&self) -> f64 {
        self.x + self.width
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
    pub fn inflate(&self, by: f64) -> Self {
        Self {
            x: self.x - by,
            y: self.y - by,
            width: self.width + 2.0 * by,
            height: self.height + 2.0 * by,
        }
    }
    pub fn valid(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite())
            && self.width >= 0.0
            && self.height >= 0.0
    }
    /// Strictly inside, so the boundary itself stays routable.
    pub fn contains_strictly(&self, point: Point) -> bool {
        point.x > self.x + EPSILON
            && point.x < self.right() - EPSILON
            && point.y > self.y + EPSILON
            && point.y < self.bottom() - EPSILON
    }
    /// Whether an axis-aligned segment passes through the open interior.
    pub fn blocks(&self, a: Point, b: Point) -> bool {
        let overlaps = |low: f64, high: f64, start: f64, end: f64| {
            low.max(start.min(end)) < high.min(start.max(end)) - EPSILON
        };
        if (a.y - b.y).abs() <= EPSILON {
            a.y > self.y + EPSILON
                && a.y < self.bottom() - EPSILON
                && overlaps(self.x, self.right(), a.x, b.x)
        } else {
            a.x > self.x + EPSILON
                && a.x < self.right() - EPSILON
                && overlaps(self.y, self.bottom(), a.y, b.y)
        }
    }
}

/// The side of a part a port sits on; a connector leaves perpendicular to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// The outward unit normal (page y grows downward).
    pub fn normal(self) -> Point {
        match self {
            Self::Left => Point::new(-1.0, 0.0),
            Self::Right => Point::new(1.0, 0.0),
            Self::Top => Point::new(0.0, -1.0),
            Self::Bottom => Point::new(0.0, 1.0),
        }
    }

    /// Which side of `bounds` the port belongs to, judged from the centre and normalized by the
    /// half extents so a long thin part (a wire, a lead) still reports its ends correctly. A port
    /// at the centre has no side.
    pub fn of(port: Point, bounds: Rect) -> Option<Self> {
        if !port.finite() || !bounds.valid() {
            return None;
        }
        let center = bounds.center();
        let relative = |delta: f64, half: f64| {
            if half > EPSILON {
                delta / half
            } else if delta.abs() > EPSILON {
                delta.signum() * f64::INFINITY
            } else {
                0.0
            }
        };
        let nx = relative(port.x - center.x, bounds.width / 2.0);
        let ny = relative(port.y - center.y, bounds.height / 2.0);
        if nx.abs() < 0.05 && ny.abs() < 0.05 {
            return None;
        }
        Some(if nx.abs() >= ny.abs() {
            if nx < 0.0 { Self::Left } else { Self::Right }
        } else if ny < 0.0 {
            Self::Top
        } else {
            Self::Bottom
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sides_of_parts_wires_and_centres() {
        let resistor = Rect {
            x: 100.0,
            y: 50.0,
            width: 48.0,
            height: 16.0,
        };
        assert_eq!(
            Side::of(Point::new(100.0, 58.0), resistor),
            Some(Side::Left)
        );
        assert_eq!(
            Side::of(Point::new(148.0, 58.0), resistor),
            Some(Side::Right)
        );
        assert_eq!(Side::of(Point::new(124.0, 50.0), resistor), Some(Side::Top));
        assert_eq!(Side::of(resistor.center(), resistor), None);
        let wire = Rect {
            x: 10.0,
            y: 10.0,
            width: 0.0,
            height: 40.0,
        };
        assert_eq!(Side::of(Point::new(10.0, 10.0), wire), Some(Side::Top));
        assert_eq!(Side::of(Point::new(10.0, 50.0), wire), Some(Side::Bottom));
        let flat = Rect {
            height: 0.0,
            width: 40.0,
            ..wire
        };
        assert_eq!(Side::of(Point::new(50.0, 10.0), flat), Some(Side::Right));
    }

    #[test]
    fn blocking_uses_the_open_interior() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
        };
        assert!(rect.blocks(Point::new(-5.0, 5.0), Point::new(15.0, 5.0)));
        assert!(!rect.blocks(Point::new(-5.0, 0.0), Point::new(15.0, 0.0)));
        assert!(!rect.blocks(Point::new(10.0, -5.0), Point::new(10.0, 15.0)));
        assert!(rect.blocks(Point::new(5.0, -5.0), Point::new(5.0, 1.0)));
        assert!(!rect.blocks(Point::new(5.0, -5.0), Point::new(5.0, 0.0)));
    }
}
