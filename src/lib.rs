pub mod bytes;
pub mod journal;
pub mod nand;

use core::mem::size_of;
use bytes::{dhara_r32, dhara_w32};
use journal::{DharaJournal, DHARA_MAX_RETRIES, DHARA_META_SIZE, DHARA_PAGE_NONE};
use nand::{DharaNand, DharaPage};

// Types

/// The map is a journal indexing format.  It maps virtual sectors to
/// pages of data in flash memory.
pub type DharaSector = u32;

// Constants
// This sector value is reserved.
const DHARA_SECTOR_NONE: DharaSector = 0xffffffff;  // TODO: if we have Option/Result return types, do we need this?
const DHARA_RADIX_DEPTH: usize = size_of::<DharaSector>() << 3;

// TODO: possible move to a new module, to include human-readable functions.
#[derive(Debug,PartialEq)]
pub enum DharaError {
    BadBlock,
    ECC,
    TooBad,
    Recover,
    JournalFull,
    NotFound,
    MapFull,
    CorruptMap,
    Max,        // TODO: do we need "max", because Rust knows how many are in an enum?
}

/// Generics:
/// N: The number of bytes on a NAND flash page.
pub struct DharaMap<const N: usize,T: DharaNand> {
    // TODO: Journal is public so that tests can reach in and examine it.
    //       Change that somehow?
    pub journal: journal::DharaJournal<N,T>,
    gc_ratio: u8,
    count: DharaSector,
}

// ///////////////////////////////////////////////////////////////////////
// Public interface
// ///////////////////////////////////////////////////////////////////////
//
impl<const N: usize,T: DharaNand> DharaMap<N,T> {
    // The original "init" was renamed "new" to match common Rust usage.

    /// Initialize a map. You need to supply 
    /// nand: A nand driver struct that implements the DharaNand trait.
    ///     It must have a page size that matches the constant generic N.
    /// 
    /// page_buf: A buffer of size N that the journal uses to hold page
    ///     metadata. The buffer will be owned by the map and its journal.
    /// 
    /// gc_ratio: a garbage collection ratio. This is the ratio of garbage
    ///     collection operations to real writes when automatic collection is
    ///     active. Smaller values lead to faster and more predictable IO, at
    ///     the expense of capacity. You should always initialize the same 
    ///     chip with the same garbage collection ratio.
    pub fn new(nand: T, page_buf: [u8; N], gc_ratio: u8) -> Self {
        let mut ratio: u8 = gc_ratio;
        if ratio == 0 {
            ratio = 1;
        }

        let mut journal = journal::DharaJournal::<N,T>::new(nand, page_buf);
        
        DharaMap {
            journal: journal,
            gc_ratio: ratio,
            count: 0, // This will get updated when resume() is called.
        }
    }

    /// Recover stored state, if possible. If there is no valid stored state
    /// on the chip, an error is returned, and an empty map is initialized.
    pub fn resume(&mut self) -> Result<(), DharaError> {
        match self.journal.journal_resume() {
            Err(e) => {
                self.count = 0;
                Err(e)
            },
            Ok(_) => {
                self.count = self.journal.get_cookie();
                Ok(())
            },
        }
    }

    /// Clear the map (delete all sectors).
    pub fn clear(&mut self) -> () {
        if self.count != 0 {
            self.count = 0;
            self.journal.journal_clear();
        }
    }

    // Renamed functions from dhara_map_capacity() and dhara_map_size()
    // to get_capacity() and get_size() to reflect their actions.

    /// Obtain the maximum capacity of the map.
    /// This might be zero if amounts reserved for garbage collection
    /// and a safety margin exceed the journal's capacity.
    pub fn get_capacity(&self) -> DharaSector {
        let cap = self.journal.journal_capacity();
        let reserve = cap / (self.gc_ratio as u32 + 1);
        let safety_margin = (DHARA_MAX_RETRIES as u32) << self.journal.nand.get_log2_ppb();

        cap.saturating_sub(reserve + safety_margin)
    }

    /// Obtain the current number of allocated sectors.
    pub fn get_size(&self) -> DharaSector {
        self.count
    }

    /// Find the physical page which holds the current data for this sector.
    /// If the sector does not exist, the error will be DharaError::NotFound.
    pub fn find(&mut self, target: DharaSector) -> Result<DharaPage, DharaError> {
        let mut unused: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];
        self.trace_path(target, &mut unused)
    }

    /// Read from the given logical sector. If the sector is unmapped, a
    /// blank page (0xff) will be returned.
    /// TODO: Should we say anything about the size of the slice?
    pub fn read(&mut self, sector: DharaSector, data: &mut [u8]) -> Result<(), DharaError> {
        match self.find(sector) {
            Err(DharaError::NotFound) => {
                data.fill(0xFF);
                Ok(())
            },
            Err(e) => Err(e),
            Ok(page) => self.journal.nand.read(page, 0, 1usize << self.journal.nand.get_log2_page_size(), data),
        }
    }

    /// Write data to a logical sector.
    /// TODO: can this be a partial write, or if not, specify that data must be a full page long.
    pub fn write(&mut self, dst: DharaSector, data: &[u8]) -> Result<(), DharaError> {
        let mut meta: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];

        loop {
            let old_count = self.count;

            self.prepare_write(dst, &mut meta)?;

            match self.journal.journal_enqueue(Some(data), Some(&meta)) {
                Ok(_) => {return Ok(());},
                Err(e) => {
                    self.count = old_count;
                    self.try_recover(e)?; // Breaks/returns on error.
                }
            }
        }
    }

    /// Copy any flash page to a logical sector.
    pub fn copy_page(&mut self, src_page: DharaPage, dst_sector: DharaSector) -> Result<(), DharaError> {
        let mut meta: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];

        loop {
            let old_count = self.count;

            self.prepare_write(dst_sector, &mut meta)?;

            match self.journal.journal_copy(src_page, Some(&meta)) {
                Ok(_) => {return Ok(());},
                Err(e) => {
                    self.count = old_count;
                    self.try_recover(e)?; // Breaks/returns on error.
                }
            }
        }
    }

    /// Copy one sector to another. If the source sector is unmapped, the
    /// destination sector will be trimmed.
    pub fn copy_sector(&mut self, src: DharaSector, dst: DharaSector) -> Result<(), DharaError> {
        match self.find(src) {
            Err(DharaError::NotFound) => self.trim(dst),
            Err(e) => Err(e),
            Ok(page) => self.copy_page(page, dst),
        }
    }

    /// Delete a logical sector. You don't necessarily need to do this, but
    /// it's a useful hint if you no longer require the sector's data to be
    /// kept.
    pub fn trim(&mut self, sector: DharaSector) -> Result<(), DharaError> {
        loop {
            self.auto_gc()?;
            match self.try_delete(sector) {
                Ok(_) => {return Ok(());},
                Err(e) => {
                    self.try_recover(e)?;
                }
            }
        }
    }

    /// Synchronize the map. Once this returns successfully, all changes to
    /// date are persistent and durable. Conversely, there is no guarantee
    /// that unsynchronized changes will be persistent.
    pub fn sync(&mut self) -> Result<(), DharaError> {
        while !self.journal.journal_is_clean() {
            let p = self.journal.journal_peek();
            let mut ret: Result<(),DharaError>;

            if p == DHARA_PAGE_NONE {
                ret = self.pad_queue();
            } else {
                ret = self.raw_gc(p);
                if ret.is_ok() {
                    self.journal.journal_dequeue();
                }
            }

            match ret {
                Ok(_) => (),
                Err(e) => {
                    self.try_recover(e)?;
                },
            }
        }
        Ok(())
    }

    /// Perform one garbage collection step. You can do this whenever you
    /// like, but it's not necessary -- garbage collection happens
    /// automatically and is interleaved with other operations.
    pub fn gc(&mut self) -> Result<(), DharaError> {
        if self.count == 0 {
            return Ok(());
        }

        loop {
            let tail = self.journal.journal_peek();

            if tail == DHARA_PAGE_NONE {
                break;
            }

            match self.raw_gc(tail) {
                Ok(_) => {
                    self.journal.journal_dequeue();
                    break;
                },
                Err(e) => {
                    self.try_recover(e)?;
                }
            }
        }
        Ok(())
    } 

}

// ///////////////////////////////////////////////////////////////////////
// Private methods
// ///////////////////////////////////////////////////////////////////////
//
impl<const N: usize,T: DharaNand> DharaMap<N,T> {

    // Trace the path from the root to the given sector, emitting
    // alt-pointers and alt-full bits in the given metadata buffer. This
    // also returns the physical page containing the given sector, if it
    // exists.
    // 
    // If the page can't be found, a suitable path will be constructed
    // (containing PAGE_NONE alt-pointers), mutating new_meta, and 
    // DHARA_E_NOT_FOUND will be returned.
    //
    // The C code passes in pointers to buffer, page location, and error code.
    // The buffer and page location can be NULL, in which case they will not
    // be written.  This saves allocating a DHARA_META_SIZE buffer in one case
    // where the function is called (out of 4).  For simplicity, this version
    // requires that you always allocate that buffer.  The return value is 
    // a Result, containing either the page value or an error.  In no case in
    // the original C code is the page value used if an error is returned,
    // thouth the mutated buffer is used when prepare_write() calls
    // trace_path() and swallows the error in certain situations.
    //
    // Also, the C code uses a goto to exit in some errors, and I've elected
    // to have a function call take care of it.  If inlined, it will be the same.
    fn trace_path(&mut self, target: DharaSector, new_meta: &mut [u8]) -> Result<DharaPage, DharaError> {
        let mut meta: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];
        let mut depth: usize = 0;
        let mut p = self.journal.get_root();

        meta_set_id(new_meta, target);

        if p == DHARA_PAGE_NONE {
            return trace_not_found(new_meta, depth);
        }

        self.journal.journal_read_meta(p, &mut meta)?;

        while depth < DHARA_RADIX_DEPTH {
            let id = meta_get_id(&meta);

            if id == DHARA_SECTOR_NONE {
                return trace_not_found(new_meta, depth);
            }

            if (target ^ id) & d_bit(depth) != 0 {
                meta_set_alt(new_meta, depth, p);
                p = meta_get_alt(&meta, depth);

                if p == DHARA_PAGE_NONE {
                    depth += 1;
                    return trace_not_found(new_meta, depth);
                }

                self.journal.journal_read_meta(p, &mut meta)?;
            } else {
                let value = meta_get_alt(&meta, depth);
                meta_set_alt(new_meta, depth, value);
            }
            depth += 1;
        }
        Ok(p)
    }

    // Check the given page. If it's garbage, do nothing. Otherwise, rewrite
    // it at the front of the map. Return raw errors from the journal (do
    // not perform recovery).
    fn raw_gc(&mut self, src: DharaPage) -> Result<(),DharaError> {
        let mut meta: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];

        // Get meta and return if error.
        self.journal.journal_read_meta(src, &mut meta)?;

        // Is the page just filler/garbage?
        let target = meta_get_id(&meta);
        if target == DHARA_SECTOR_NONE {
            return Ok(());
        }

        // Find out where the sector once represented by this page
        // currently resides (if anywhere).
        match self.trace_path(target, &mut meta) {
            Err(DharaError::NotFound) => Ok(()),
            Err(e) => Err(e),
            Ok(current_page) => {
                // Is this page still the most current representative?
                // If not, do nothing.
                if current_page != src {
                    return Ok(());
                }

                // Rewrite it at the front of the journal with updated metadata.
                self.journal.set_cookie(self.count);
                self.journal.journal_copy(src, Some(&meta))?;
                Ok(())
            },
        }
    }

    fn pad_queue(&mut self) -> Result<(),DharaError> {
        let p = self.journal.get_root();
        let mut root_meta: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];

        self.journal.set_cookie(self.count);

        if p == DHARA_PAGE_NONE {
            return self.journal.journal_enqueue(None, None);
        }

        self.journal.journal_read_meta(p, &mut root_meta)?;

        return self.journal.journal_copy(p, Some(&root_meta));
    }

    // Attempt to recover the journal.
    fn try_recover(&mut self, cause: DharaError) -> Result<(),DharaError> {
        if cause != DharaError::Recover {
            return Err(cause);
        }

        let mut restart_count: u8 = 0;

        while self.journal.journal_in_recovery() {
            let mut ret: Result<(),DharaError>;
            let p = self.journal.journal_next_recoverable();

            if p == DHARA_PAGE_NONE {
                ret = self.pad_queue();
            } else {
                ret = self.raw_gc(p);
            }

            match ret {
                Ok(_) => {continue;},
                Err(DharaError::Recover) => {
                    if restart_count >= DHARA_MAX_RETRIES {
                        return Err(DharaError::TooBad);
                    }
                    restart_count += 1;
                },
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn auto_gc(&mut self) -> Result<(),DharaError> {
        if self.journal.journal_size() < self.get_capacity() {
            return Ok(());
        }

        for _ in 0..self.gc_ratio {
            self.gc()?;
        }
        Ok(())
    }

    fn prepare_write(&mut self, dst: DharaSector, meta: &mut [u8]) -> Result<(),DharaError> {
        self.auto_gc()?;  // Collect garbage and return if error.

        match self.trace_path(dst, meta) {
            Ok(_) => (),
            Err(DharaError::NotFound) => {
                if self.count >= self.get_capacity() {
                    return Err(DharaError::MapFull);
                }
                self.count += 1;
            },
            Err(e) => {return Err(e);},
        }
        self.journal.set_cookie(self.count);
        Ok(())
    }

    fn try_delete(&mut self, sector: DharaSector) -> Result<(),DharaError> {
        let mut meta: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];
        let mut alt_meta: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];
        let mut level = DHARA_RADIX_DEPTH - 1;
        let mut alt_page: DharaPage;

        // The value of this expression is the return value of the function.
        match self.trace_path(sector, &mut meta) {
            Err(DharaError::NotFound) => Ok(()),
            Err(e) => Err(e),
            Ok(_) => {
                // Select any of the closest cousins of this node which are
                // subtrees of at least the requested order.
                loop {
                    alt_page = meta_get_alt(&meta, level);
                    if alt_page != DHARA_PAGE_NONE {
                        break;
                    }

                    level -= 1;
                    
                    // Special case: deletion of last sector
                    if level == 0 {
                        self.count = 0;
                        self.journal.journal_clear();
                        return Ok(());
                    }
                }

                // Rewrite the cousin with an up-to-date path which doesn't
                // point to the original node.
                self.journal.journal_read_meta(alt_page, &mut alt_meta)?;

                meta_set_id(&mut meta, meta_get_id(&alt_meta));

                meta_set_alt(&mut meta, level, DHARA_PAGE_NONE);
                for i in (level+1)..DHARA_RADIX_DEPTH {
                    meta_set_alt(&mut meta, i, meta_get_alt(&alt_meta, i));
                }
                meta_set_alt(&mut meta, level, DHARA_PAGE_NONE); // TODO: is this statement redundant?

                self.journal.set_cookie(self.count - 1);

                self.journal.journal_copy(alt_page, Some(&meta))?;  // TODO: document why this function takes an Option.

                self.count -= 1;
                Ok(())
            },
        }
    }

}

// ///////////////////////////////////////////////////////////////////////
// Helper functions
// ///////////////////////////////////////////////////////////////////////
//
// Note: I omitted meta_clear() because it was unused.

pub fn meta_get_id(meta: &[u8]) -> DharaSector {
    dhara_r32(&meta[0..4])
}

fn meta_set_id(meta: &mut [u8], value: DharaSector) -> () {
    dhara_w32(&mut meta[0..4], value);
}

// Get an alt-pointer.
// level: the depth of the pointer in the tree.
pub fn meta_get_alt(meta: &[u8], level: usize) -> DharaPage {
    let idx = 4 + (level << 2);
    dhara_r32(&meta[idx..idx+4])
}

// Set an alt-pointer.
// level: the depth of the pointer in the tree.
fn meta_set_alt(meta: &mut [u8], level: usize, alt: DharaPage) -> () {
    let idx = 4 + (level << 2);
    dhara_w32(&mut meta[idx..idx+4], alt);
}

fn d_bit(depth: usize) -> DharaSector {
    let temp: DharaSector = 1;
    temp << (DHARA_RADIX_DEPTH - depth - 1)
}

fn trace_not_found(new_meta: &mut [u8], mut depth: usize) -> Result<DharaPage, DharaError> {
    while depth < DHARA_RADIX_DEPTH {
        meta_set_alt(new_meta, depth, DHARA_SECTOR_NONE);
        depth += 1;
    }
    Err(DharaError::NotFound)
}

// fn trace_path(target: DharaSector, new_meta: &mut Option<&mut [u8]>) -> Result<DharaPage, DharaError> {
//     // let mut meta: [u8; DHARA_META_SIZE] = [0; DHARA_META_SIZE];
//     // let mut depth: usize = 0;
//     // let p = self.journal.get_root();

//     new_meta.map(|s| s[0] = 1);
//     Ok(0)
// }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        // let mut meta: [u8;5] = [0;5];
        // let mut meta2 = Some(&mut meta);
        // trace_path(2, &mut meta2);
        // assert_eq!(meta[0], 1);
    }
}
