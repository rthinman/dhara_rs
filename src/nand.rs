// NAND driver.  You need to edit/supply this file.

use crate::DharaError;

// Each page in a NAND device is indexed, starting at 0. It's required
// that there be a power-of-two number of pages in a eraseblock, so you can
// view a page number is being a concatenation (in binary) of a block
// number and the number of a page within a block.
pub type DharaPage  = u32;

// Blocks are also indexed, starting at 0.
pub type DharaBlock = u32;

pub const NAND_PAGE_SIZE: usize = 2048;
pub const NAND_SPARE_SIZE: usize = 64;

// pub struct DharaNand {
//     pub log2_page_size: u8,
//     pub log2_ppb: u8,
//     pub num_blocks: u32,

// }

/// Each NAND chip must be represented by a structure that implements
/// this trait.
pub trait DharaNand {
    /// Get the base-2 logarithm of the page size. If your device supports
    /// partial programming, you may want to subdivide the actual
    /// pages into separate ECC-correctable regions and present those
    /// as pages.
    fn get_log2_page_size(&self) -> u8;

    /// Get the base-2 logarithm of the number of pages within an erase block.
    fn get_log2_ppb(&self) -> u8;

    /// Get the total number of erase blocks.
    fn get_num_blocks(&self) -> u32;  // TODO: change to usize?

    /// Is the given block bad?
    fn is_bad(&self, blk: DharaBlock) -> bool;

    /// Mark the given block as bad (or attempt to).  No return value is
    /// required, because there's nothing that can be done in response.
    fn mark_bad(&self, blk: DharaBlock) -> ();

    /// Erase the given block.  This function should return Ok(0) on success
    /// or Err(e) on failure.  The status reported by the chip should
    /// be checked.  If an erase operation fails, it should return 
    /// Err(BadBlock).
    fn erase(&self, blk: DharaBlock) -> Result<u8,DharaError>;

    /// Program the given page.  
    /// The data pointer is *** TODO figure this out.
    /// The operation status should be checked.  If the operation fails,
    /// return Err(BadBlock).
    /// Pages will be programmed sequentially within a block, and will
    /// not be reprogrammed.
    fn prog(&self, page: DharaPage, data: &[u8]) -> Result<u8,DharaError>;

    /// Check the the given page is erased.
    fn is_free(&self, page: DharaPage) -> bool;

    /// Read a portion of a page. ECC must be handled by the NAND 
    /// implementation. Returns Ok(0) on sucess or Err(e) if an error occurs. 
    /// If an uncorrectable ECC error occurs, return Err(ECC).
    // TODO: is this the right way to handle errors?  The u8 isn't really used.
    // TODO: is this the right way to deal with data? Check this reads into an external slice.
    fn read(&self, page: u32, offset: usize, length: usize, data: &mut[u8]) -> Result<u8, DharaError>;

    /// Read a page from one location and reprogram it in another location.
    /// This might be done using the chip's internal buffers, but it must use
    /// ECC.
    fn copy(&self, src: DharaPage, dst: DharaPage) -> Result<u8,DharaError>;
}