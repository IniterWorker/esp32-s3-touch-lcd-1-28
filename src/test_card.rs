//! Pure drawing core: the reference test card and rotation marker.
//!
//! No ESP32-S3 (or even GC9A01) dependencies — anything that is a
//! `DrawTarget<Color = Rgb565>` renders the identical pixels. That is what
//! makes the blocking and async firmware bins comparable, and what the host
//! probe-point tests pin down.

use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{Point, Size},
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::{Rgb565, RgbColor},
    prelude::{Drawable, Primitive},
    primitives::{Circle, Line, PrimitiveStyle, Rectangle, Triangle},
    text::{Alignment, Text},
};

/// Panel geometry of the GC9A01 on this board.
pub const WIDTH: i32 = 240;
/// Panel geometry of the GC9A01 on this board.
pub const HEIGHT: i32 = 240;
const CENTER: Point = Point::new(WIDTH / 2, HEIGHT / 2);

/// Mid gray used for the eighth color bar (Rgb565 has no named gray).
pub const GRAY: Rgb565 = Rgb565::new(16, 32, 16);

/// The eight reference colors of the bar row, left to right.
pub const COLOR_BARS: [Rgb565; 8] = [
    Rgb565::RED,
    Rgb565::GREEN,
    Rgb565::BLUE,
    Rgb565::CYAN,
    Rgb565::MAGENTA,
    Rgb565::YELLOW,
    Rgb565::WHITE,
    GRAY,
];

const BAR_X: i32 = 24;
const BAR_Y: i32 = 56;
const BAR_W: u32 = 24;
const BAR_H: u32 = 28;

/// Draw the full reference test card (240x240).
///
/// Composition, in draw order (later shapes paint over earlier ones):
/// black background; two corner-to-corner diagonals; the 8-bar color row;
/// four 16x16 quadrant squares (R/G/B/W); a 1px border inset by 4px plus
/// the inscribed circle (radius 118) so the visible round area is obvious;
/// a filled center circle with a crosshair; `label` and the fixed
/// `GC9A01 0.5 240x240` caption below center.
///
/// # Errors
///
/// Returns the draw target's error on the first failed draw.
pub fn draw_test_card<D>(target: &mut D, label: &str) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    target.clear(Rgb565::BLACK)?;

    let thin_white = PrimitiveStyle::with_stroke(Rgb565::WHITE, 1);

    // Diagonals first: bars, squares and text paint over them.
    Line::new(Point::new(8, 8), Point::new(232, 232))
        .into_styled(thin_white)
        .draw(target)?;
    Line::new(Point::new(232, 8), Point::new(8, 232))
        .into_styled(thin_white)
        .draw(target)?;

    // Color bars.
    for (i, color) in COLOR_BARS.iter().enumerate() {
        Rectangle::new(
            Point::new(BAR_X + i as i32 * BAR_W as i32, BAR_Y),
            Size::new(BAR_W, BAR_H),
        )
        .into_styled(PrimitiveStyle::with_fill(*color))
        .draw(target)?;
    }

    // Quadrant squares: TL red, TR green, BL blue, BR white.
    let squares = [
        (Point::new(28, 28), Rgb565::RED),
        (Point::new(196, 28), Rgb565::GREEN),
        (Point::new(28, 196), Rgb565::BLUE),
        (Point::new(196, 196), Rgb565::WHITE),
    ];
    for (pos, color) in squares {
        Rectangle::new(pos, Size::new(16, 16))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(target)?;
    }

    // Border + inscribed circle (the panel is round; corners are invisible
    // on real hardware, the circle shows exactly what is).
    Rectangle::new(Point::new(4, 4), Size::new(232, 232))
        .into_styled(thin_white)
        .draw(target)?;
    Circle::with_center(CENTER, 236)
        .into_styled(thin_white)
        .draw(target)?;

    // Center filled circle + crosshair on top of it.
    Circle::with_center(CENTER, 24)
        .into_styled(PrimitiveStyle::with_fill(Rgb565::CYAN))
        .draw(target)?;
    Line::new(
        Point::new(CENTER.x - 20, CENTER.y),
        Point::new(CENTER.x + 20, CENTER.y),
    )
    .into_styled(thin_white)
    .draw(target)?;
    Line::new(
        Point::new(CENTER.x, CENTER.y - 20),
        Point::new(CENTER.x, CENTER.y + 20),
    )
    .into_styled(thin_white)
    .draw(target)?;

    // Captions.
    let text_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    Text::with_alignment(
        label,
        Point::new(CENTER.x, 158),
        text_style,
        Alignment::Center,
    )
    .draw(target)?;
    Text::with_alignment(
        "GC9A01 0.5 240x240",
        Point::new(CENTER.x, 172),
        text_style,
        Alignment::Center,
    )
    .draw(target)?;

    Ok(())
}

/// Draw an unambiguous "this way is up" marker: a big filled arrow pointing
/// to the top of the frame, with `label` (e.g. `ROT90`) above it. On the
/// round panel the arrow direction is the rotation proof.
///
/// # Errors
///
/// Returns the draw target's error on the first failed draw.
pub fn draw_rotation_marker<D>(target: &mut D, label: &str) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    target.clear(Rgb565::BLACK)?;

    let fill_white = PrimitiveStyle::with_fill(Rgb565::WHITE);

    // Arrowhead.
    Triangle::new(
        Point::new(CENTER.x, 56),
        Point::new(CENTER.x - 44, 124),
        Point::new(CENTER.x + 44, 124),
    )
    .into_styled(fill_white)
    .draw(target)?;
    // Shaft.
    Rectangle::new(Point::new(CENTER.x - 12, 124), Size::new(24, 64))
        .into_styled(fill_white)
        .draw(target)?;

    Text::with_alignment(
        label,
        Point::new(CENTER.x, 34),
        MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE),
        Alignment::Center,
    )
    .draw(target)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use embedded_graphics::{
        Pixel,
        draw_target::DrawTarget,
        geometry::{OriginDimensions, Size},
    };

    // A minimal 240x240 in-memory target. embedded-graphics' MockDisplay is
    // hard-limited to 64x64 (const SIZE in mock_display), so it cannot hold
    // this card; this stands in for it with the same probe-point queries.
    struct TestDisplay {
        pixels: [Rgb565; (WIDTH * HEIGHT) as usize],
    }

    impl TestDisplay {
        fn new() -> Self {
            Self {
                pixels: [Rgb565::BLACK; (WIDTH * HEIGHT) as usize],
            }
        }

        fn get(&self, x: i32, y: i32) -> Rgb565 {
            self.pixels[(y * WIDTH + x) as usize]
        }
    }

    impl OriginDimensions for TestDisplay {
        fn size(&self) -> Size {
            Size::new(WIDTH as u32, HEIGHT as u32)
        }
    }

    impl DrawTarget for TestDisplay {
        type Color = Rgb565;
        type Error = Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Infallible>
        where
            I: IntoIterator<Item = Pixel<Rgb565>>,
        {
            for Pixel(pos, color) in pixels {
                if pos.x >= 0 && pos.x < WIDTH && pos.y >= 0 && pos.y < HEIGHT {
                    self.pixels[(pos.y * WIDTH + pos.x) as usize] = color;
                }
            }
            Ok(())
        }

        fn clear(&mut self, color: Rgb565) -> Result<(), Infallible> {
            self.pixels.fill(color);
            Ok(())
        }
    }

    #[test]
    fn test_card_probe_points() {
        let mut d = TestDisplay::new();
        draw_test_card(&mut d, "BLOCKING").unwrap();

        // Background away from every shape stays black.
        assert_eq!(d.get(120, 220), Rgb565::BLACK);
        assert_eq!(d.get(20, 120), Rgb565::BLACK);

        // Border (inset 4). The inscribed circle's top (y ~ 120 - 118)
        // lands on one of a few pixels depending on rasterization of the
        // even diameter, so probe the small neighborhood instead of one
        // exact pixel.
        assert_eq!(d.get(4, 120), Rgb565::WHITE);
        assert!(
            (116..=124).any(|x| d.get(x, 2) == Rgb565::WHITE || d.get(x, 3) == Rgb565::WHITE),
            "inscribed circle top not drawn"
        );

        // Every color bar at its center.
        for (i, expected) in COLOR_BARS.iter().enumerate() {
            let x = BAR_X + i as i32 * BAR_W as i32 + BAR_W as i32 / 2;
            assert_eq!(d.get(x, BAR_Y + BAR_H as i32 / 2), *expected, "bar {i}");
        }

        // Quadrant squares at their centers.
        assert_eq!(d.get(36, 36), Rgb565::RED);
        assert_eq!(d.get(204, 36), Rgb565::GREEN);
        assert_eq!(d.get(36, 204), Rgb565::BLUE);
        assert_eq!(d.get(204, 204), Rgb565::WHITE);

        // Diagonal pixel away from overpainted shapes.
        assert_eq!(d.get(170, 170), Rgb565::WHITE);

        // Crosshair owns the exact center; the filled circle shows off-axis.
        assert_eq!(d.get(CENTER.x, CENTER.y), Rgb565::WHITE);
        assert_eq!(d.get(CENTER.x + 6, CENTER.y - 6), Rgb565::CYAN);
    }

    #[test]
    fn rotation_marker_arrow_points_up() {
        let mut d = TestDisplay::new();
        draw_rotation_marker(&mut d, "ROT0").unwrap();

        // Inside the arrowhead and the shaft.
        assert_eq!(d.get(120, 90), Rgb565::WHITE);
        assert_eq!(d.get(120, 160), Rgb565::WHITE);
        // Above the tip and off to the side of the shaft: background.
        assert_eq!(d.get(120, 48), Rgb565::BLACK);
        assert_eq!(d.get(40, 160), Rgb565::BLACK);
        // Below the shaft a down-arrow would paint; an up-arrow does not.
        assert_eq!(d.get(120, 210), Rgb565::BLACK);
    }
}
