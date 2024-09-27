use esp_idf_hal::gpio::{AnyIOPin, PinDriver};
use esp_idf_hal::task::block_on;
use esp_idf_hal::{delay::Delay, i2c};
use qmi8658::command::register::acceleration::{AccelerationOutput, AngularRateOutput};
use qmi8658::command::register::ctrl1::{Ctrl1Register, IntDirection};
use qmi8658::command::register::ctrl2::{AccelerometerFS, AccelerometerODR, Ctrl2Register};
use qmi8658::command::register::ctrl3::{Ctrl3Register, GyroscopeFS, GyroscopeODR};
use qmi8658::command::register::ctrl7::Ctrl7Register;
use qmi8658::command::register::ctrl8::Ctrl8Register;
use qmi8658::command::register::fifo_ctrl::{FIFOMode, FIFOSize};
use qmi8658::Qmi8658;
use shared_bus::BusManager;
use std::sync::{Arc, Mutex};

pub struct SensorsTaskData<'a> {
    pub shared_orientation: Arc<Mutex<Orientation>>,
    pub delay: Delay,
    pub bus: &'a BusManager<Mutex<i2c::I2cDriver<'a>>>,
    pub _int1: AnyIOPin,
    pub int2: AnyIOPin,
}

pub fn sensors_setup(
    qmi8658_device: &mut Qmi8658<shared_bus::I2cProxy<'_, Mutex<i2c::I2cDriver<'_>>>, Delay>,
) -> anyhow::Result<()> {
    let mut ctrl1 = Ctrl1Register(0);
    ctrl1.set_int1_enable(true);
    ctrl1.set_be(true); // Data Big-Endian
    ctrl1.set_int2_enable(true);

    if let Err(e) = qmi8658_device.set_ctrl1(ctrl1) {
        log::error!("QMI8658 write set_ctrl1 error: {:?}", e);
    }

    let mut ctrl2 = Ctrl2Register(0);
    ctrl2.set_afs(AccelerometerFS::FS2G);
    ctrl2.set_aodr(AccelerometerODR::NormalAODR8);
    ctrl2.set_ast(false);

    if let Err(e) = qmi8658_device.set_ctrl2(ctrl2) {
        log::error!("QMI8658 write set_ctrl2 error: {:?}", e);
    }

    let mut ctrl3: Ctrl3Register = Ctrl3Register(0);
    ctrl3.set_godr(GyroscopeODR::NormalGORD8);
    ctrl3.set_gfs(GyroscopeFS::DPS256);
    ctrl3.set_gst(false);

    if let Err(e) = qmi8658_device.set_ctrl3(ctrl3) {
        log::error!("QMI8658 write set_ctrl7 error: {:?}", e);
    }

    // let mut ctrl5: Ctrl5Register = Ctrl5Register(0);
    // ctrl5

    let mut ctrl7: Ctrl7Register = Ctrl7Register(0);
    ctrl7.set_gyroscope_enable(true);
    ctrl7.set_accelerometer_enable(true);
    ctrl7.set_sync_sample_enable(false);
    ctrl7.set_data_ready_disable(true);

    if let Err(e) = qmi8658_device.set_ctrl7(ctrl7) {
        log::error!("QMI8658 write set_ctrl7 error: {:?}", e);
    }

    let mut ctrl8: Ctrl8Register = Ctrl8Register(0);
    ctrl8.set_ctrl9_handshake_type(true);

    if let Err(e) = qmi8658_device.set_ctrl8(ctrl8) {
        log::error!("QMI8658 write set_ctrl7 error: {:?}", e);
    }

    log::info!("Setup sensors task started!");

    qmi8658_device
        .config_fifo(FIFOMode::Fifo, FIFOSize::Size128, IntDirection::Int2, 64)
        .unwrap();

    anyhow::Result::Ok(())
}

pub fn gyroscope_task(data: SensorsTaskData) -> anyhow::Result<()> {
    let mut int2 = PinDriver::input(data.int2)?;

    let bus = data.bus.acquire_i2c();

    let mut gyroscope = Qmi8658::new_secondary_address(bus, data.delay);

    let _ = match gyroscope.get_device_id() {
        Ok(rev) => {
            log::info!("QMI8658 Device ID: {:?}", rev);
            anyhow::Result::Ok(())
        }
        Err(err) => {
            log::error!("QMI8658 not found");
            anyhow::Result::Err(err)
        }
    };

    let _ = match gyroscope.get_device_revision_id() {
        Ok(rev) => {
            log::info!("QMI8658 Device Revision ID: {:x}", rev);
            anyhow::Result::Ok(())
        }
        Err(err) => {
            log::error!("QMI8658 not found");
            anyhow::Result::Err(err)
        }
    };

    let is_gyro_not_dead = match gyroscope.gyroscope_test() {
        Ok(()) => {
            log::info!("Gyroscope SelfTest Okay");
            true
        }
        Err(err) => {
            log::error!("Gyroscope SelfTest Error: {:?}", err);
            false
        }
    };

    let is_acc_not_dead = match gyroscope.accelerometer_test() {
        Ok(()) => {
            log::info!("Accelerometer SelfTest Okay");
            true
        }
        Err(err) => {
            log::error!("Accelerometer SelfTest Error: {:?}", err);
            false
        }
    };

    let shared = data.shared_orientation.lock();
    if let Ok(mut shared) = shared {
        shared.is_gyro_not_dead = is_gyro_not_dead;
        shared.is_acc_not_dead = is_acc_not_dead;
    }

    sensors_setup(&mut gyroscope)?;

    loop {
        let result = block_on(int2.wait_for_rising_edge());

        if result.is_ok() {
            if let Err(err) = gyroscope.read_fifo_data(|idx, acc, gyro| {
                let shared = data.shared_orientation.lock();
                if let Ok(mut shared) = shared {
                    shared.update(idx, gyro, acc)
                }
            }) {
                log::error!("Gyroscoper reading error: {:?}", err);
            }
        } else if let Err(err) = result {
            log::error!("waiting on interupt error: {}", err)
        }
    }
}

// Define a struct to hold the current orientation in degrees for each axis
#[derive(Debug, Default, Clone, Copy)]
pub struct Orientation {
    pub is_gyro_not_dead: bool,
    pub is_acc_not_dead: bool,
    pub idx: u16,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub x_acc: f32,
    pub y_acc: f32,
    pub z_acc: f32,
}

impl Orientation {
    // Function to update the orientation based on angular rate
    fn update(
        &mut self,
        idx: u16,
        angular_rate: Option<AngularRateOutput>,
        acc: Option<AccelerationOutput>,
    ) {
        // Update the current orientation
        self.idx = idx;
        self.x = angular_rate.unwrap_or_default().x;
        self.y = angular_rate.unwrap_or_default().y;
        self.z = angular_rate.unwrap_or_default().z;
        self.x_acc = acc.unwrap_or_default().x;
        self.y_acc = acc.unwrap_or_default().y;
        self.z_acc = acc.unwrap_or_default().z;
    }
}
