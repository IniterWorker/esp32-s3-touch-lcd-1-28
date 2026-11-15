//! Sandbox settings: pure codec (host-testable) + flash-backed store.
//!
//! On-flash layout (32 bytes, little-endian):
//!
//! ```text
//!  0..4   magic "SBX1"
//!  4      version = 1
//!  5..9   seq u32 (monotonic; newest valid slot wins)
//!  9      backlight_pct
//! 10      rotation (0..3 → 0/90/180/270°)
//! 11      invert (0/1)
//! 12      imu_enabled (0/1)
//! 13      pedometer (0/1)
//! 14      touch_enabled (0/1)
//! 15      stream_imu (0/1)
//! 16..18  sleep_timeout_s u16
//! 18      imu_odr_idx
//! 19..28  reserved (0)
//! 28..32  CRC-32 (IEEE) over bytes 0..28
//! ```
//!
//! The store keeps two 4 KiB slots in the `settings` flash partition
//! (see `partitions.csv`) and alternates between them, so a power cut
//! mid-save leaves the previous settings readable.

/// User-facing sandbox settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Backlight duty, percent.
    pub backlight_pct: u8,
    /// Display rotation index: 0..3 → 0/90/180/270°.
    pub rotation: u8,
    /// Invert panel colors.
    pub invert: bool,
    /// IMU sampling enabled.
    pub imu_enabled: bool,
    /// Pedometer engine enabled.
    pub pedometer: bool,
    /// Legacy touch-enabled flag. Touch is always on (a persisted
    /// `false` once stranded a user with no input path back); the
    /// field stays only for on-flash record compatibility and is
    /// normalized to `true` on load. Do not add a UI/UART toggle.
    pub touch_enabled: bool,
    /// Stream IMU samples over UART.
    pub stream_imu: bool,
    /// Seconds without touch before the backlight blanks (0 = never).
    pub sleep_timeout_s: u16,
    /// IMU output-data-rate selection index (meaning defined by the
    /// sandbox sensor task).
    pub imu_odr_idx: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            backlight_pct: 100,
            rotation: 2, // Rotate180 — this board's long-standing default
            invert: false,
            imu_enabled: true,
            pedometer: true,
            touch_enabled: true,
            stream_imu: false,
            sleep_timeout_s: 60,
            imu_odr_idx: 0,
        }
    }
}

const MAGIC: [u8; 4] = *b"SBX1";
const VERSION: u8 = 1;
/// Encoded record length.
pub const RECORD_LEN: usize = 32;

/// CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320), table-less.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// Encode `settings` with sequence number `seq` into a 32-byte record.
pub fn encode(seq: u32, s: &Settings) -> [u8; RECORD_LEN] {
    let mut buf = [0u8; RECORD_LEN];
    buf[0..4].copy_from_slice(&MAGIC);
    buf[4] = VERSION;
    buf[5..9].copy_from_slice(&seq.to_le_bytes());
    buf[9] = s.backlight_pct;
    buf[10] = s.rotation;
    buf[11] = u8::from(s.invert);
    buf[12] = u8::from(s.imu_enabled);
    buf[13] = u8::from(s.pedometer);
    buf[14] = u8::from(s.touch_enabled);
    buf[15] = u8::from(s.stream_imu);
    buf[16..18].copy_from_slice(&s.sleep_timeout_s.to_le_bytes());
    buf[18] = s.imu_odr_idx;
    let crc = crc32(&buf[..28]);
    buf[28..32].copy_from_slice(&crc.to_le_bytes());
    buf
}

/// Decode a record; `None` on bad magic/version/CRC or short input.
pub fn decode(buf: &[u8]) -> Option<(u32, Settings)> {
    if buf.len() < RECORD_LEN || buf[0..4] != MAGIC || buf[4] != VERSION {
        return None;
    }
    let stored = u32::from_le_bytes(buf[28..32].try_into().ok()?);
    if crc32(&buf[..28]) != stored {
        return None;
    }
    Some((
        u32::from_le_bytes(buf[5..9].try_into().ok()?),
        Settings {
            backlight_pct: buf[9],
            rotation: buf[10],
            invert: buf[11] != 0,
            imu_enabled: buf[12] != 0,
            pedometer: buf[13] != 0,
            touch_enabled: buf[14] != 0,
            stream_imu: buf[15] != 0,
            sleep_timeout_s: u16::from_le_bytes(buf[16..18].try_into().ok()?),
            imu_odr_idx: buf[18],
        },
    ))
}

#[cfg(feature = "board")]
pub use store::SettingsStore;

#[cfg(feature = "board")]
mod store {
    use esp_bootloader_esp_idf::partitions::read_partition_table;
    use esp_storage::FlashStorage;
    use static_cell::StaticCell;

    use super::{RECORD_LEN, Settings, decode, encode};

    /// Partition-table scratch buffer. Exactly
    /// `PARTITION_TABLE_MAX_LEN` (0xC00): `PartitionTable::new`
    /// rejects longer inputs. Claimed once by [`SettingsStore::open`].
    static PT_BUF: StaticCell<[u8; 0xC00]> = StaticCell::new();

    const SLOT_SIZE: u32 = 0x1000;
    /// Fallback if the partition table has no `settings` label —
    /// mirrors `partitions.csv`; keep the two in sync.
    const SETTINGS_OFFSET: u32 = 0x9000;

    /// Two-slot settings store in the `settings` flash partition.
    pub struct SettingsStore<'d> {
        flash: FlashStorage<'d>,
        base: u32,
        /// Slot index (0/1) holding the newest valid record, if any.
        newest: Option<u32>,
        seq: u32,
        current: Settings,
    }

    impl<'d> SettingsStore<'d> {
        /// Open the store: locate the `settings` partition by label
        /// (fallback: the `partitions.csv` offset constant), then scan
        /// both slots. `None` only if the partition table itself is
        /// unreadable.
        pub fn open(mut flash: FlashStorage<'d>) -> Option<Self> {
            let buf = PT_BUF.init([0u8; 0xC00]);
            let base = match read_partition_table(&mut flash, buf) {
                Ok(pt) => pt
                    .iter()
                    .find(|e| e.label_as_str() == "settings")
                    .map(|e| e.offset())
                    .unwrap_or(SETTINGS_OFFSET),
                Err(_) => return None,
            };
            let mut store = Self {
                flash,
                base,
                newest: None,
                seq: 0,
                current: Settings::default(),
            };
            store.rescan();
            Some(store)
        }

        fn read_slot(&mut self, slot: u32) -> Option<(u32, Settings)> {
            let mut buf = [0u8; RECORD_LEN];
            self.flash
                .read(self.base + slot * SLOT_SIZE, &mut buf)
                .ok()?;
            decode(&buf)
        }

        fn rescan(&mut self) {
            self.newest = None;
            self.seq = 0;
            self.current = Settings::default();
            for slot in 0..2 {
                if let Some((seq, s)) = self.read_slot(slot)
                    && (self.newest.is_none() || seq > self.seq)
                {
                    self.newest = Some(slot);
                    self.seq = seq;
                    self.current = s;
                }
            }
            // Touch is always on; heal records saved by builds that
            // still had the toggle (see field docs).
            self.current.touch_enabled = true;
        }

        /// Newest valid `(seq, settings)`, or `(0, Settings::default())`
        /// when no slot decodes.
        pub fn load(&self) -> (u32, Settings) {
            (self.seq, self.current)
        }

        /// Whether any valid record exists on flash.
        pub fn has_stored(&self) -> bool {
            self.newest.is_some()
        }

        /// Save with `seq + 1` into the other slot (erase-then-write).
        /// Returns the new sequence number.
        ///
        /// # Errors
        ///
        /// Returns the flash error on erase/write failure; in-memory
        /// state is left unchanged.
        pub fn save(&mut self, settings: &Settings) -> Result<u32, esp_storage::FlashStorageError> {
            let slot = match self.newest {
                Some(s) => 1 - s,
                None => 0,
            };
            let seq = self.seq.wrapping_add(1);
            let record = encode(seq, settings);
            // `Storage::write` does read-modify-erase-write per sector,
            // which is exactly the slot semantics wanted here.
            self.flash.write(self.base + slot * SLOT_SIZE, &record)?;
            self.newest = Some(slot);
            self.seq = seq;
            self.current = *settings;
            Ok(seq)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_defaults() {
        let s = Settings::default();
        assert_eq!(decode(&encode(7, &s)), Some((7, s)));
    }

    #[test]
    fn roundtrip_all_fields() {
        let s = Settings {
            backlight_pct: 42,
            rotation: 1,
            invert: true,
            imu_enabled: false,
            pedometer: false,
            touch_enabled: false,
            stream_imu: true,
            sleep_timeout_s: 3600,
            imu_odr_idx: 3,
        };
        assert_eq!(decode(&encode(u32::MAX, &s)), Some((u32::MAX, s)));
    }

    #[test]
    fn bad_magic_rejected() {
        let mut buf = encode(1, &Settings::default());
        buf[0] = b'X';
        assert_eq!(decode(&buf), None);
    }

    #[test]
    fn flipped_payload_bit_rejected_by_crc() {
        let mut buf = encode(1, &Settings::default());
        buf[9] ^= 0x01;
        assert_eq!(decode(&buf), None);
    }

    #[test]
    fn erased_flash_rejected() {
        assert_eq!(decode(&[0xFF; RECORD_LEN]), None);
    }

    #[test]
    fn crc32_known_vector() {
        // Standard CRC-32 check value for "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
