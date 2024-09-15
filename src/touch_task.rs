use cst816s::command::{IrqCtl, KeyEvent, MotionMask};
use cst816s::Cst816s;
use esp_idf_hal::gpio::{AnyIOPin, OutputPin, PinDriver};
use esp_idf_hal::task::block_on;
use esp_idf_hal::{delay::Delay, i2c};
use shared_bus::BusManager;
use std::sync::{Arc, Mutex};

pub struct TouchTaskData<'a> {
    pub shared_cursor: Arc<Mutex<KeyEvent>>,
    pub delay: Delay,
    pub bus: &'a BusManager<Mutex<i2c::I2cDriver<'a>>>,
    pub int1: AnyIOPin,
    pub reset: AnyIOPin,
}

pub fn setup_touch(
    touch: &mut Cst816s<shared_bus::I2cProxy<'_, Mutex<i2c::I2cDriver<'_>>>, Delay>,
) -> anyhow::Result<()> {
    let mut irq_ctl = IrqCtl(0);
    irq_ctl.set_en_test(false);
    irq_ctl.set_en_touch(true);
    irq_ctl.set_en_change(true);
    irq_ctl.set_en_motion(true);
    irq_ctl.set_en_once_wlp(true);
    touch.write_irq_ctl(irq_ctl)?;

    let mut motion_mask = MotionMask(0);
    motion_mask.set_en_double_click(true);
    motion_mask.set_en_continuous_left_right(true);
    motion_mask.set_en_continuous_up_down(true);
    touch.write_motion_mask(motion_mask)?;

    touch.write_lp_scan_idac(1)?;
    touch.write_lp_scan_freq(7)?;
    touch.write_lp_scan_win(3)?;
    touch.write_lp_scan_th(48)?;
    touch.write_motion_s1_angle(0)?;
    touch.write_long_press_time(10)?;
    touch.write_auto_reset(5)?;

    Ok(())
}

pub fn touch_task(mut data: TouchTaskData) -> anyhow::Result<()> {
    let mut int1 = PinDriver::input(data.int1)?;
    let mut reset_output = PinDriver::output(data.reset.downgrade_output())?;

    let bus = data.bus.acquire_i2c();
    let mut touch = Cst816s::new(bus, data.delay);

    touch.reset(&mut reset_output, &mut data.delay)?;

    setup_touch(&mut touch)?;

    touch.dump_register();

    loop {
        let result = block_on(int1.wait_for_rising_edge());

        let event = touch.read_events();

        if let Ok(event) = event {
            if let Some(key) = event.report_key::<240, 240>() {
                let mut value = data.shared_cursor.lock().unwrap();
                *value = key;
            }
        }

        if result.is_ok() {
        } else if let Err(err) = result {
            log::error!("waiting on interupt error: {}", err)
        }
    }
}
