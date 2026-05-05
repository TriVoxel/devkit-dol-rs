//! File-as-block-device bridge.
//!
//! [`FileImage`] wraps any open `FsFile` and exposes it as a [`BlockDev`],
//! allowing filesystem drivers to read an `.iso` image stored inside another
//! filesystem (e.g. a PS1 ISO on a FAT32 SD card).
//!
//! ## Example
//!
//! ```rust,no_run
//! // Mount SD card with FAT32
//! let vol = vfs::mount_sd(Slot::A, FsKind::Auto)?;
//! let iso_file = vol.open("/ROMS/game.iso")?;
//!
//! // Wrap the file as a block device (2048-byte sectors for ISO 9660)
//! let image = FileImage::new(iso_file, 2048);
//!
//! // Mount the image as an ISO 9660 volume
//! let iso_vol = vfs::mount_image(image, FsKind::Iso9660)?;
//! ```

use crate::{FsError, Result, BlockDev};

/// A file opened from any mounted filesystem, used as a block device.
///
/// The concrete file state is stored inline (no heap allocation) up to
/// `STORAGE` bytes. The sector size is configurable — use 512 for FAT
/// images, 2048 for ISO 9660 / GC DVD images.
pub struct FileImage<const SECTOR: usize, D: BlockDev> {
    dev:     D,
    /// Starting byte offset of the image within the device
    /// (for images embedded inside a partition or file).
    offset:  u64,
    /// Image size in bytes (0 = use full device).
    size:    u64,
}

impl<const SECTOR: usize, D: BlockDev> FileImage<SECTOR, D> {
    /// Create an image starting at `offset` bytes into `dev` with size `size`.
    ///
    /// Pass `size = 0` to use the full device.
    pub fn new(dev: D, offset: u64, size: u64) -> Self {
        FileImage { dev, offset, size }
    }

    /// Create an image covering the full device from byte 0.
    pub fn whole(dev: D) -> Self {
        FileImage { dev, offset: 0, size: 0 }
    }

    /// Total image size in bytes.
    fn image_size(&self) -> u64 {
        if self.size > 0 { self.size }
        else { self.dev.sector_size() as u64 * self.dev.sector_count() }
    }
}

impl<const SECTOR: usize, D: BlockDev> BlockDev for FileImage<SECTOR, D> {
    fn sector_size(&self) -> usize { SECTOR }

    fn sector_count(&self) -> u64 {
        self.image_size() / SECTOR as u64
    }

    unsafe fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<()> {
        if buf.len() < SECTOR { return Err(FsError::BufferTooSmall); }

        let image_byte = self.offset + lba * SECTOR as u64;

        // We need to translate the image byte offset to device sectors.
        // The underlying device may have a different sector size.
        let dev_ss   = self.dev.sector_size() as u64;
        let dev_lba  = image_byte / dev_ss;
        let dev_off  = (image_byte % dev_ss) as usize;

        if dev_off == 0 && SECTOR % self.dev.sector_size() == 0 {
            // Perfectly aligned: read directly
            // We might need to read multiple device sectors per image sector
            let dev_sectors = SECTOR / self.dev.sector_size();
            let mut tmp = [0u8; 4096]; // enough for 2 × 2048 or 8 × 512
            let tmp_len = (dev_sectors * self.dev.sector_size()).min(4096);
            for i in 0..dev_sectors {
                let chunk = &mut tmp[i * self.dev.sector_size()..(i+1) * self.dev.sector_size()];
                self.dev.read_sector(dev_lba + i as u64, chunk)?;
            }
            buf[..SECTOR].copy_from_slice(&tmp[..SECTOR]);
        } else {
            // Unaligned: read into temp buffer and extract
            let mut tmp = [0u8; 4096];
            let needed_bytes = dev_off + SECTOR;
            let dev_sectors = (needed_bytes + self.dev.sector_size() - 1)
                            / self.dev.sector_size();
            let read_bytes = (dev_sectors * self.dev.sector_size()).min(4096);
            let _ = read_bytes;
            for i in 0..dev_sectors {
                let chunk_size = self.dev.sector_size();
                let chunk = &mut tmp[i * chunk_size..(i+1) * chunk_size];
                self.dev.read_sector(dev_lba + i as u64, chunk)?;
            }
            buf[..SECTOR].copy_from_slice(&tmp[dev_off..dev_off + SECTOR]);
        }
        Ok(())
    }

    unsafe fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<()> {
        if buf.len() < SECTOR { return Err(FsError::BufferTooSmall); }
        let image_byte = self.offset + lba * SECTOR as u64;
        let dev_ss  = self.dev.sector_size() as u64;
        let dev_lba = image_byte / dev_ss;
        let dev_sectors = SECTOR / self.dev.sector_size();
        for i in 0..dev_sectors {
            let off = i * self.dev.sector_size();
            let chunk: &[u8; 512] = buf[off..off + 512].try_into()
                .map_err(|_| FsError::BufferTooSmall)?;
            self.dev.write_sector(dev_lba + i as u64,
                &buf[off..off + self.dev.sector_size()])?;
        }
        Ok(())
    }
}
