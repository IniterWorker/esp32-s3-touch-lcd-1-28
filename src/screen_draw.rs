use cst816s::command::KeyEvent;
use embedded_graphics::{
    framebuffer::Framebuffer,
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::{
        raw::{LittleEndian, RawU16},
        Rgb565,
    },
    prelude::{Point, Primitive, RgbColor},
    primitives::{Circle, PrimitiveStyleBuilder},
    text::Text,
    Drawable,
};

use arrayvec::ArrayString;
use core::fmt::Write;

use gc9a01::{mode::BufferedGraphics, prelude::*, Gc9a01};

use crate::gyroscope_task::Orientation;

pub struct DrawEngine<'a> {
    style: MonoTextStyle<'a, Rgb565>,
    buffer: ArrayString<32>,
}

#[derive(Default)]
pub struct DrawContext {
    pub orientation: Orientation,
    pub cursor: KeyEvent,
    pub counter: u32,
    pub counter_lock: u32,
}

impl<'a> DrawEngine<'a> {
    pub fn new() -> Self {
        Self {
            style: MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE),
            buffer: ArrayString::new(),
        }
    }

    pub fn draw<
        I: WriteOnlyDataCommand,
        D: DisplayDefinition<Buffer = Framebuffer<Rgb565, RawU16, LittleEndian, 240, 240, 115200>>,
    >(
        &mut self,
        display: &mut Gc9a01<I, D, BufferedGraphics<D>>,
        context: &DrawContext,
    ) {
        let (w, _h) = display.dimensions();
        let w = w as i32;
        // let h = h as i32;

        self.buffer.clear();
        // Draw x value
        let _ = write!(&mut self.buffer, "Gyro.x:{:.2}", context.orientation.x);
        Text::new(self.buffer.as_str(), Point::new(w / 2, 20), self.style)
            .draw(display)
            .unwrap();

        self.buffer.clear();
        // Draw y value
        let _ = write!(&mut self.buffer, "Gyro.y:{:.2}", context.orientation.y);
        Text::new(self.buffer.as_str(), Point::new(w / 2, 20 + 20), self.style)
            .draw(display)
            .unwrap();

        self.buffer.clear();
        // Draw z value
        let _ = write!(&mut self.buffer, "Gyro.z:{:.2}", context.orientation.z);
        Text::new(self.buffer.as_str(), Point::new(w / 2, 20 + 40), self.style)
            .draw(display)
            .unwrap();

        self.buffer.clear();
        // Draw x value
        let _ = write!(&mut self.buffer, "Acc.x:{:.2}", context.orientation.x_acc);
        Text::new(self.buffer.as_str(), Point::new(w / 2, 65), self.style)
            .draw(display)
            .unwrap();

        self.buffer.clear();
        // Draw y value
        let _ = write!(&mut self.buffer, "Acc.y:{:.2}", context.orientation.y_acc);
        Text::new(self.buffer.as_str(), Point::new(w / 2, 65 + 20), self.style)
            .draw(display)
            .unwrap();

        self.buffer.clear();
        // Draw z value
        let _ = write!(&mut self.buffer, "Acc.z:{:.2}", context.orientation.z_acc);
        Text::new(self.buffer.as_str(), Point::new(w / 2, 65 + 40), self.style)
            .draw(display)
            .unwrap();
        self.buffer.clear();
        // Draw z value
        let _ = write!(&mut self.buffer, "idx:{:}", context.orientation.idx);
        Text::new(self.buffer.as_str(), Point::new(w / 2, 65 + 60), self.style)
            .draw(display)
            .unwrap();
        self.buffer.clear();
        let _ = write!(&mut self.buffer, "counter_lock:{:}", context.counter_lock);
        Text::new(self.buffer.as_str(), Point::new(10, 65 + 20), self.style)
            .draw(display)
            .unwrap();
        self.buffer.clear();
        let _ = write!(&mut self.buffer, "counter:{:}", context.counter);
        Text::new(self.buffer.as_str(), Point::new(10, 65 + 40), self.style)
            .draw(display)
            .unwrap();
        // Draw not dead
        self.buffer.clear();
        let _ = write!(
            &mut self.buffer,
            "act:{:}",
            context.orientation.is_gyro_not_dead
        );
        Text::new(self.buffer.as_str(), Point::new(10, 65 + 60), self.style)
            .draw(display)
            .unwrap();
        // Draw not dead
        self.buffer.clear();
        let _ = write!(
            &mut self.buffer,
            "act:{:}",
            context.orientation.is_gyro_not_dead
        );
        Text::new(self.buffer.as_str(), Point::new(10, 65 + 80), self.style)
            .draw(display)
            .unwrap();
        // Draw X
        self.buffer.clear();
        let _ = write!(&mut self.buffer, "x:{:}", context.cursor.x);
        Text::new(self.buffer.as_str(), Point::new(10, 65 + 100), self.style)
            .draw(display)
            .unwrap();
        // Draw X
        self.buffer.clear();
        let _ = write!(&mut self.buffer, "y:{:}", context.cursor.y);
        Text::new(self.buffer.as_str(), Point::new(10, 65 + 120), self.style)
            .draw(display)
            .unwrap();

        let style = PrimitiveStyleBuilder::new().fill_color(Rgb565::RED).build();

        Circle::new(
            Point::new(
                i32::from(context.cursor.x) - 5,
                i32::from(context.cursor.y) - 5,
            ),
            5,
        )
        .into_styled(style)
        .draw(display)
        .unwrap()
    }
}
