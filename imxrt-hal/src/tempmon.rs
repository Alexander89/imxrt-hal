//! # Temperature Monitor (TEMPMON)
//!
//! ## IMPORTANT NOTE:
//! The temperature sensor uses and assumes that the bandgap
//! reference, 480MHz PLL and 32KHz RTC modules are properly programmed and fully
//! settled for correct operation.
//!
//!
//! ## Example 1
//!
//! Manually triggered read
//!
//! ```no_run
//! use imxrt_hal;
//!
//! let mut peripherals = imxrt_hal::Peripherals::take().unwrap();
//!
//! let (_, ipg_hz) = peripherals.ccm.pll1.set_arm_clock(
//!     imxrt_hal::ccm::PLL1::ARM_HZ,
//!     &mut peripherals.ccm.handle,
//!     &mut peripherals.dcdc,
//! );
//!
//! let mut cfg = peripherals.ccm.perclk.configure(
//!     &mut peripherals.ccm.handle,
//!     imxrt_hal::ccm::perclk::PODF::DIVIDE_3,
//!     imxrt_hal::ccm::perclk::CLKSEL::IPG(ipg_hz),
//! );
//!
//! // init temperature monitor
//! let mut temp_mon = peripherals.tempmon.init();
//! loop {
//!     if let Ok(temperature) = nb::block!(temp_mon.measure_temp()) {
//!         // temperature in mC (1°C = 1000°mC)
//!     }
//! }
//! ```
//!
//! ## Example 2
//!
//! Non-blocking reading
//!
//! ```no_run
//! use imxrt_hal::{self, tempmon::TempMon};
//!
//! # let mut peripherals = imxrt_hal::Peripherals::take().unwrap();
//! # let (_, ipg_hz) = peripherals.ccm.pll1.set_arm_clock(
//! #    imxrt_hal::ccm::PLL1::ARM_HZ,
//! #    &mut peripherals.ccm.handle,
//! #    &mut peripherals.dcdc,
//! # );
//! # let mut cfg = peripherals.ccm.perclk.configure(
//! #    &mut peripherals.ccm.handle,
//! #    imxrt_hal::ccm::perclk::PODF::DIVIDE_3,
//! #    imxrt_hal::ccm::perclk::CLKSEL::IPG(ipg_hz),
//! # );
//!
//! // Init temperature monitor with 8Hz measure freq
//! // 0xffff = 2 Sec. Read more at `measure_freq()`
//! let mut temp_mon = peripherals.tempmon.init_with_measure_freq(0x1000);
//! temp_mon.start();
//!
//! let mut last_temp = 0_i32;
//! loop {
//!     // Get the last temperature read by the measure_freq
//!     if let Ok(temp) = temp_mon.get_temp() {
//!         if last_temp != temp {
//!             // temperature changed
//!             last_temp = temp;
//!         }
//!         // do something else
//!     }
//! }
//! ```
//!
//! ## Example 3
//!
//! Low and high temperature Interrupt
//!
//! *NOTE*: TEMP_LOW_HIGH is triggered for `TempSensor low` and `TempSensor high`
//!
//! ```no_run
//! use imxrt_hal::{self, tempmon::TempMon};
//! use imxrt_hal::ral::interrupt;
//!
//! # let mut peripherals = imxrt_hal::Peripherals::take().unwrap();
//! # let (_, ipg_hz) = peripherals.ccm.pll1.set_arm_clock(
//! #    imxrt_hal::ccm::PLL1::ARM_HZ,
//! #    &mut peripherals.ccm.handle,
//! #    &mut peripherals.dcdc,
//! # );
//! # let mut cfg = peripherals.ccm.perclk.configure(
//! #    &mut peripherals.ccm.handle,
//! #    imxrt_hal::ccm::perclk::PODF::DIVIDE_3,
//! #    imxrt_hal::ccm::perclk::CLKSEL::IPG(ipg_hz),
//! # );
//!
//! // init temperature monitor with 8Hz measure freq
//! // 0xffff = 2 Sec. Read more at `measure_freq()`
//! let mut temp_mon = peripherals.tempmon.init_with_measure_freq(0x1000);
//!
//! // set low_alarm, high_alarm, and panic_alarm temperature
//! temp_mon.set_alarm_values(-5_000, 65_000, 95_000);
//!
//! // use values from registers if you like to compare it somewhere
//! let (low_alarm, high_alarm, panic_alarm) = temp_mon.alarm_values();
//!
//! // enables interrupts for low_high_alarm
//! unsafe {
//!     cortex_m::peripheral::NVIC::unmask(interrupt::TEMP_LOW_HIGH);
//! }
//!
//! // start could fail if the module is not powered up
//! if temp_mon.start().is_err() {
//!     temp_mon.power_up();
//!     temp_mon.start();
//! }
//!
//! #[cortex_m_rt::interrupt]
//! fn TEMP_LOW_HIGH() {
//!     // disable the interrupt to avoid endless triggers
//!     cortex_m::peripheral::NVIC::mask(interrupt::TEMP_LOW_HIGH);
//!
//!     // don't forget to enable it after the temperature is back to normal
//! }
//! ```

use crate::ral;

/// Indicates that the temperature monitor is powered down.
///
/// If you receive this error, `power_up()` the temperature monitor first,
/// and try again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerDownError(());

/// An Uninitialized temperature monitor module
///
/// # Important note:
///
/// The temperature sensor uses and assumes that the bandgap
/// reference, 480MHz PLL and 32KHz RTC modules are properly
/// programmed and fully settled for correct operation.
pub struct Uninitialized(ral::tempmon::Instance);

impl Uninitialized {
    /// assign the tempmon Instance to this temperature monitor wrapper.
    pub fn new(base: ral::tempmon::Instance) -> Self {
        Self(base)
    }

    /// Initialize the temperature monitor.
    pub fn init(self) -> TempMon {
        // this operation is safe. This value is read-only and set by the manufacturer.
        let calibration = unsafe { ral::read_reg!(ral::ocotp, OCOTP, ANA1) };

        // The ral doesn't provide direct access to the values.
        let n1_room_count = (calibration >> 20) as i32;
        let t1_room_temp = 25_000_i32;
        let n2_hot_count = ((calibration >> 8) & 0xFFF) as i32;
        let t2_hot_temp = (calibration & 0xFF) as i32 * 1_000;

        // Tmeas = HOT_TEMP - (Nmeas - HOT_COUNT) * ((HOT_TEMP - 25.0) / (ROOM_COUNT – HOT_COUNT))
        let scaler = (t2_hot_temp - t1_room_temp) / (n1_room_count - n2_hot_count);
        // Tmeas = HOT_TEMP - (Nmeas - HOT_COUNT) * scaler

        let mut t = TempMon {
            base: self.0,
            scaler,
            hot_count: n2_hot_count,
            hot_temp: t2_hot_temp,
        };
        t.power_up();
        t
    }

    /// Initialize the temperature monitor.
    ///
    /// The `measure_freq` determines how many RTC clocks to wait before automatically repeating a temperature
    /// measurement. The pause time before remeasuring is the field value multiplied by the RTC period.
    ///
    /// Find more details `TempMon.set_measure_frequency`
    pub fn init_with_measure_freq(self, measure_freq: u16) -> TempMon {
        let mut t = self.init();
        t.set_measure_frequency(measure_freq);
        t
    }
}

/// A Temperature Monitor (TEMPMON)
///
/// # Example 1
///
/// ```no_run
/// use imxrt_hal;
///
/// let mut peripherals = imxrt_hal::Peripherals::take().unwrap();
/// let (_, ipg_hz) = peripherals.ccm.pll1.set_arm_clock(
///     imxrt_hal::ccm::PLL1::ARM_HZ,
///     &mut peripherals.ccm.handle,
///     &mut peripherals.dcdc,
/// );
/// let mut cfg = peripherals.ccm.perclk.configure(
///     &mut peripherals.ccm.handle,
///     imxrt_hal::ccm::perclk::PODF::DIVIDE_3,
///     imxrt_hal::ccm::perclk::CLKSEL::IPG(ipg_hz),
/// );
///
/// // init temperature monitor
/// // consider using init_with_measure_freq
/// let mut temp_mon = peripherals.tempmon.init();
/// loop {
///     if let Ok(_temperature) = nb::block!(temp_mon.measure_temp()) {
///         // _temperature in mC (1°C = 1000°mC)
///     }
/// }
/// ```
pub struct TempMon {
    base: ral::tempmon::Instance,
    /// scaler * 1000
    scaler: i32,
    /// hot_count * 1000
    hot_count: i32,
    /// hot_temp * 1000
    hot_temp: i32,
}

impl TempMon {
    /// converts the temp_cnt into a human readable temperature [°mC] (1/1000 °C)
    fn convert(&self, temp_cnt: i32) -> i32 {
        let n_meas = temp_cnt - self.hot_count;
        self.hot_temp - n_meas * self.scaler
    }

    /// decode the temp_value into measurable bytes
    ///
    /// param **temp_value_mc**: in °mC (1/1000°C)
    ///
    fn decode(&self, temp_value_mc: i32) -> u32 {
        let v = (temp_value_mc - self.hot_temp) / self.scaler;
        (self.hot_count - v) as u32
    }

    /// triggers a new measurement
    ///
    /// If you configured automatically repeating, this will trigger additional measurement.
    /// Use get_temp instate to get the last read value
    ///
    /// The returning temperature in 1/1000 Celsius (°mC)
    ///
    /// Example: 25500°mC -> 25.5°C
    pub fn measure_temp(&mut self) -> nb::Result<i32, PowerDownError> {
        if !self.is_powered_up() {
            Err(nb::Error::from(PowerDownError(())))
        } else {
            // if no measurement is active, trigger new measurement
            let active = ral::read_reg!(ral::tempmon, self.base, TEMPSENSE0, MEASURE_TEMP == START);
            if !active {
                ral::write_reg!(ral::tempmon, self.base, TEMPSENSE0_SET, MEASURE_TEMP: START);
            }

            // If the measurement is not finished or not started
            // i.MX Docs: This bit should be cleared by the sensor after the start of each measurement
            if ral::read_reg!(ral::tempmon, self.base, TEMPSENSE0, FINISHED == INVALID) {
                // measure_temp could be triggered again without any effect
                Err(nb::Error::WouldBlock)
            } else {
                // clear MEASURE_TEMP to trigger a new measurement at the next call
                ral::write_reg!(ral::tempmon, self.base, TEMPSENSE0_CLR, MEASURE_TEMP: START);

                let temp_cnt = ral::read_reg!(ral::tempmon, self.base, TEMPSENSE0, TEMP_CNT) as i32;
                Ok(self.convert(temp_cnt))
            }
        }
    }

    /// Returns the last read value from the temperature sensor
    ///
    /// The returning temperature in 1/1000 Celsius (°mC)
    ///
    /// Example: 25500°mC -> 25.5°C
    pub fn get_temp(&self) -> nb::Result<i32, PowerDownError> {
        if self.is_powered_up() {
            let temp_cnt = ral::read_reg!(ral::tempmon, self.base, TEMPSENSE0, TEMP_CNT) as i32;
            Ok(self.convert(temp_cnt))
        } else {
            Err(nb::Error::from(PowerDownError(())))
        }
    }

    /// Starts the measurement process. If the measurement frequency is zero, this
    /// results in a single conversion.
    pub fn start(&mut self) -> nb::Result<(), PowerDownError> {
        if self.is_powered_up() {
            ral::write_reg!(ral::tempmon, self.base, TEMPSENSE0_SET, MEASURE_TEMP: START);
            Ok(())
        } else {
            Err(nb::Error::from(PowerDownError(())))
        }
    }

    /// Stops the measurement process. This only has an effect If the measurement
    /// frequency is not zero.
    pub fn stop(&mut self) {
        ral::write_reg!(ral::tempmon, self.base, TEMPSENSE0_CLR, MEASURE_TEMP: START);
    }

    /// returns the true if the tempmon module is powered up.
    pub fn is_powered_up(&self) -> bool {
        ral::read_reg!(ral::tempmon, self.base, TEMPSENSE0, POWER_DOWN == POWER_UP)
    }

    /// This powers down the temperature sensor.
    pub fn power_down(&mut self) {
        ral::write_reg!(
            ral::tempmon,
            self.base,
            TEMPSENSE0_SET,
            POWER_DOWN: POWER_DOWN
        );
    }

    /// This powers up the temperature sensor.
    pub fn power_up(&mut self) {
        ral::write_reg!(
            ral::tempmon,
            self.base,
            TEMPSENSE0_CLR,
            POWER_DOWN: POWER_DOWN
        );
    }

    /// Set the temperature that will generate a low alarm, high alarm, and panic alarm interrupt
    /// when the temperature exceeded this values.
    ///
    /// ## Note:
    /// low_alarm_mc, high_alarm_mc, and panic_alarm_mc are in milli Celsius (1/1000 °C)
    pub fn set_alarm_values(&mut self, low_alarm_mc: i32, high_alarm_mc: i32, panic_alarm_mc: i32) {
        let low_alarm = self.decode(low_alarm_mc);
        let high_alarm = self.decode(high_alarm_mc);
        let panic_alarm = self.decode(panic_alarm_mc);
        ral::modify_reg!(ral::tempmon, self.base, TEMPSENSE0, ALARM_VALUE: high_alarm);
        ral::write_reg!(
            ral::tempmon,
            self.base,
            TEMPSENSE2,
            LOW_ALARM_VALUE: low_alarm,
            PANIC_ALARM_VALUE: panic_alarm
        );
    }

    /// Queries the temperature that will generate a low alarm, high alarm, and panic alarm interrupt.
    ///
    /// returns (low_alarm_temp, high_alarm_temp, panic_alarm_temp)
    pub fn alarm_values(&self) -> (i32, i32, i32) {
        let high_alarm = ral::read_reg!(ral::tempmon, self.base, TEMPSENSE0, ALARM_VALUE);
        let (low_alarm, panic_alarm) = ral::read_reg!(
            ral::tempmon,
            self.base,
            TEMPSENSE2,
            LOW_ALARM_VALUE,
            PANIC_ALARM_VALUE
        );
        (
            self.convert(low_alarm as i32),
            self.convert(high_alarm as i32),
            self.convert(panic_alarm as i32),
        )
    }

    /// This bits determines how many RTC clocks to wait before automatically repeating a temperature
    /// measurement. The pause time before remeasuring is the field value multiplied by the RTC period.
    ///
    /// | value  | note |
    /// | ------ | ----------------------------------------------------- |
    /// | 0x0000 | Defines a single measurement with no repeat.          |
    /// | 0x0001 | Updates the temperature value at a RTC clock rate.    |
    /// | 0x0002 | Updates the temperature value at a RTC/2 clock rate.  |
    /// | ...    | ... |
    /// | 0xFFFF | Determines a two second sample period with a 32.768KHz RTC clock. Exact timings depend on the accuracy of the RTC clock.|
    ///
    pub fn set_measure_frequency(&mut self, measure_freq: u16) {
        ral::modify_reg!(
            ral::tempmon,
            self.base,
            TEMPSENSE1,
            MEASURE_FREQ: measure_freq as u32
        );
    }
}
