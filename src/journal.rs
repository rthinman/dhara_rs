use crate::bytes::{dhara_r32, dhara_w32};
use crate::nand::{DharaBlock, DharaNand, DharaPage};
use crate::DharaError;

/// Number of bytes used by the journal checkpoint header, as well
/// as positions in the header (as laid out in map_internals.txt).
const DHARA_HEADER_SIZE: usize = 16;
const DHARA_HEADER_EPOCH_IDX: usize = 3; // One byte after the 3-byte "magic number".
const DHARA_HEADER_TAIL_IDX: usize = 4;  // 4-byte tail
const DHARA_HEADER_BBC_IDX: usize = 8;   // 4-byte Bad Block before Current head
const DHARA_HEADER_BBL_IDX: usize = 12;  // 4-byte est. total Bad Blocks

/// Global metadata available for a higher layer. This metadata is
/// persistent once the journal reaches a checkpoint, and is restored on
/// startup.
/// 
const DHARA_COOKIE_SIZE: usize = 4;

/// This is the size of the metadata slice which accompanies each written
/// page. This is independent of the underlying page/OOB size.
/// 
pub const DHARA_META_SIZE: usize = 132;

/// When a block fails, or garbage is encountered, we try again on the
/// next block/checkpoint. We can do this up to the given number of
/// times.
/// 
pub const DHARA_MAX_RETRIES: u8 = 8;

/// This is a page number which can be used to represent "no such page".
/// It's guaranteed to never be a valid user page.
/// 
pub const DHARA_PAGE_NONE: DharaPage = 0xffffffff;

// State flags
// TODO: Is there a more idiomatic way to represent this in Rust?
// bitflags crate... maybe
const DHARA_JOURNAL_F_DIRTY: u8 = 		0x01;
const DHARA_JOURNAL_F_BAD_META: u8 = 	0x02;
const DHARA_JOURNAL_F_RECOVERY: u8 = 	0x04;
const DHARA_JOURNAL_F_ENUM_DONE: u8 = 	0x08;

/// The journal layer presents the NAND pages as a double-ended queue.
/// Pages, with associated metadata may be pushed onto the end of the
/// queue, and pages may be popped from the end.
/// Block erase, metadata storage are handled automatically. Bad blocks
/// are handled by relocating data to the next available non-bad page in
/// the sequence.
/// It's up to the user to ensure that the queue doesn't grow beyond the
/// capacity of the NAND chip, but helper functions are provided to
/// assist with this. If the head meets the tail, the journal will refuse
/// to enqueue more pages.
/// 
pub struct DharaJournal<const N: usize,T: DharaNand> {
    // TODO: Need to deal with the NAND driver.
    // TODO: Made this public for jtutil's dequeue function.  Is there a 
    //       better way?  If we keep it like this, there are places where we could 
    //       clean up, like removing DharaJournal's nand parameter getters.
    /// A NAND driver implementation.
    pub nand: T, 
    
    /// The temporary buffer where page data are kept.
    page_buf: [u8; N],

	/// In the journal, user data is grouped into checkpoints of
	/// 2**log2_ppc contiguous aligned pages.
	/// 
	/// The last page of each checkpoint contains the journal header
	/// and the metadata for the other pages in the period (the user
	/// pages).
	/// 
    log2_ppc: u8, 

    /// Epoch counter. This is incremented whenever the journal head
	/// passes the end of the chip and wraps around.
	/// 
	epoch: u8, 

	/// General purpose flags field */
	flags: u8,

	/// Bad-block counters. bb_last is our best estimate of the
	/// number of bad blocks in the chip as a whole. bb_current is
	/// the number of bad blocks in all blocks before the current
	/// head.
	/// 
	bb_current: DharaBlock,
	bb_last: DharaBlock,

	/// Log head and tail. The tail pointer points to the last user
	/// page in the log, and the head pointer points to the next free
	/// raw page. The root points to the last written user page.
	/// 
	tail_sync: DharaPage,
	tail: DharaPage,
	head: DharaPage,

	/// This points to the last written user page in the journal
	root: DharaPage,

	/// Recovery mode: recover_root points to the last valid user
	/// page in the block requiring recovery. recover_next points to
	/// the next user page needing recovery.
	/// 
	/// If we had buffered metadata before recovery started, it will
	/// have been dumped to a free page, indicated by recover_meta.
	/// If this block later goes bad, we will have to defer bad-block
	/// marking until recovery is complete (F_BAD_META).
	/// 
	recover_next: DharaPage,
	recover_root: DharaPage,
	recover_meta: DharaPage,
}

// ///////////////////////////////////////////////////////////////////////
// Public interface
// ///////////////////////////////////////////////////////////////////////
//
impl<const N: usize,T: DharaNand> DharaJournal<N,T> {

    // The original "init" was renamed "new" to match common Rust usage.
    // TODO: go back to "init" because we want to statically allocate
    // a struct, and thus don't want to be passing in dynamically allocated stuff?

    /// Initialize a journal. You must supply a NAND chip
    /// driver, and a single page buffer. This page buffer will be used
    /// exclusively by the journal, but you are responsible for allocating
    /// it, and freeing it (if necessary) at the end.
    /// No NAND operations are performed at this point.
    /// 
    pub fn new(nand: T, page_buf: [u8; N]) -> Self {
        // Get these values before moving nand into the struct.
        let psize = nand.get_log2_page_size();
        let max = nand.get_log2_ppb();

        let mut j = DharaJournal::<N,T> {
            nand: nand,
            page_buf: page_buf,
            log2_ppc: choose_ppc(psize, max),
            epoch: 0,
            flags: 0,
            bb_current: 0,
            bb_last: 0,  // Gets updated in reset_journal().
            tail_sync: 0,
            tail: 0,
            head: 0,
            root: DHARA_PAGE_NONE,
            recover_next: 0,
            recover_root: 0,
            recover_meta: 0,
        };

        j.reset_journal();

        j
    }

    /// Start up the journal -- search the NAND for the journal head, or
    /// initialize a blank journal if one isn't found. Returns Ok(0) on success
    /// or Err() if a (fatal) error occurs.
    /// 
    /// This operation is O(log N), where N is the number of pages in the
    /// NAND chip. All other operations are O(1).
    /// 
    /// If this operation fails, the journal will be reset to an empty state.
    pub fn journal_resume(&mut self) -> Result<(),DharaError> {
        let res = self.find_checkblock(0);
        match res {
            Err(e) => {
                self.reset_journal();
                Err(e)
            }
            Ok(first) => {
                // Find the last checkpoint-containing block in this epoch.
                self.epoch = self.hdr_get_epoch();
                let last = self.find_last_checkblock(first);
                // Find the last programmed checkpoint group in the block.
                let last_group = self.find_last_group(last);
                // Perform a linear scan to find the last good checkpoint
                // (and therefore the root), setting self.root in the process.
                if let Err(e) = self.find_root(last_group) {
                    self.reset_journal();
                    return Err(e);
                }

                // Restore setting from the checkpoint.
                self.tail = self.hdr_get_tail();
                self.bb_current = self.hdr_get_bb_current();
                self.bb_last = self.hdr_get_bb_last();
                self.hdr_clear_user(self.nand.get_log2_page_size() as usize);

                // Perform another linear scan to find the next free user page.
                // Note that the C code checked for errors and reset the journal
                // if they happened.  But find_head() only ever returned 0.
                // Thus for now, just execute find_head().
                self.find_head(last_group);

                self.flags = 0;
                self.tail_sync = self.tail;

                self.clear_recovery();
                Ok(())
            }
        }
    }

    /// Obtain an upper bound on the number of user pages storable in the
    /// journal.
    pub fn journal_capacity(&self) -> DharaPage {
        let max_bad: DharaBlock = if self.bb_last < self.bb_current {
            self.bb_last 
        } else {
            self.bb_current
        };
        let good_blocks: DharaBlock = self.nand.get_num_blocks() - max_bad - 1;
        let log2_cpb = self.nand.get_log2_ppb() - self.log2_ppc;
        let good_cps: DharaPage = good_blocks << log2_cpb;

        // Good checkpoints * (checkpoint period -1)
        (good_cps << self.log2_ppc) - good_cps
    }

    /// Obtain an upper bound on the number of user pages consumed by the
    /// journal.
    pub fn journal_size(&self) -> DharaPage {
        // Find the number of raw pages, and the number of checkpoints
        // between the head and tail.  The difference between the two
        // is the number of user pages (upper limit).
        let mut num_pages = self.head;
        let mut num_cps = self.head >> self.log2_ppc;

        if self.head < self.tail_sync {
            let total_pages: DharaPage = self.nand.get_num_blocks() << self.nand.get_log2_ppb();
            num_pages += total_pages;
            num_cps += total_pages >> self.log2_ppc;
        }

        num_pages -= self.tail_sync;
        num_cps -= self.tail_sync >> self.log2_ppc;

        num_pages - num_cps
    }

    /// Get the "cookie" data, a global metadata location for the map layer.
    pub fn get_cookie(&self) -> u32 {
        dhara_r32(&self.page_buf[DHARA_HEADER_SIZE..(DHARA_HEADER_SIZE+DHARA_COOKIE_SIZE)])
    }

    /// Set the "cookie" data, a global metadata location for the map layer.
    pub fn set_cookie(&mut self, value: u32) -> () {
        dhara_w32(&mut self.page_buf[DHARA_HEADER_SIZE..(DHARA_HEADER_SIZE+DHARA_COOKIE_SIZE)], value);
    }

    /// Obtain the locations of the first and last pages in the journal.
    pub fn journal_root(&self) -> DharaPage {
        self.root
    }

    /// Read metadata associated with a page. This assumes that the page
    /// provided is a valid data page. The actual page data is read via the
    /// normal NAND interface.
    pub fn journal_read_meta(&mut self, page: DharaPage, buf: &mut [u8]) -> Result<(),DharaError> {
        // Offset of metadata within the metadata page
        let ppc_mask: DharaPage = (1 << self.log2_ppc) - 1;
        let offset = self.hdr_user_offset(page & ppc_mask);

        // Special case: buffered metadata
        if align_eq(page, self.head, self.log2_ppc) {
            buf[..DHARA_META_SIZE].copy_from_slice(&self.page_buf[offset..offset+DHARA_META_SIZE]);
            return Ok(());
        }

        // Special case: incomplete metadata dumped at start of recovery
        if (self.recover_meta != DHARA_PAGE_NONE) 
                && align_eq(page, self.recover_root, self.log2_ppc) {
            return self.nand.read(self.recover_meta, offset, DHARA_META_SIZE, buf);
        }

        // General case: fetch from metadata page for checkpoint group
        return self.nand.read(page | ppc_mask, offset, DHARA_META_SIZE, buf);
    }

    /// Advance the tail to the next non-bad block and return the page that's
    /// ready to read. If no page is ready, return DHARA_PAGE_NONE.
    pub fn journal_peek(&mut self) -> DharaPage {
        if self.head == self.tail {
            return DHARA_PAGE_NONE;
        }

        if is_aligned(self.tail, self.nand.get_log2_ppb()) {
            let mut block: DharaBlock = self.tail >> self.nand.get_log2_ppb();

            for _ in 0..DHARA_MAX_RETRIES {
                if (block == (self.head >> self.nand.get_log2_ppb())) 
                        || !self.nand.is_bad(block) {
                    self.tail = block << self.nand.get_log2_ppb();
                    if self.tail == self.head {
                        self.root = DHARA_PAGE_NONE;
                    }
                    return self.tail;
                }
                block = self.next_block(block);
            }
        }
        return self.tail;
    }

    /// Remove the last page from the journal. This doesn't take permanent
    /// effect until the next checkpoint.
    pub fn journal_dequeue(&mut self) -> () {
        if self.head == self.tail {
            return;
        }

        self.tail = self.next_upage(self.tail);

        // If the journal is clean at the time of dequeue, then this
        // data was always obsolete, and can be reused immediately.
        if (self.flags & (DHARA_JOURNAL_F_DIRTY | DHARA_JOURNAL_F_RECOVERY)) == 0 {
            self.tail_sync = self.tail;
        }

        let chip_size: DharaPage = self.nand.get_num_blocks() << self.nand.get_log2_ppb();
        let raw_size: DharaPage = wrap(self.head + chip_size - self.tail, chip_size);
        let root_offset: DharaPage = wrap(self.head + chip_size - self.root, chip_size);

        if root_offset > raw_size {
            self.root = DHARA_PAGE_NONE;
        }
    }

    /// Remove all pages from the journal. This doesn't take permanent effect
    /// until the next checkpoint.
    pub fn journal_clear(&mut self) -> () {
        self.tail = self.head;
        self.root = DHARA_PAGE_NONE;
        self.flags |= DHARA_JOURNAL_F_DIRTY;

        self.hdr_clear_user(self.nand.get_log2_page_size() as usize);
    }

    /// Append a page to the journal. Both raw page data and metadata must be
    /// specified. The push operation is not persistent until a checkpoint is
    /// reached.
    /// 
    /// This operation may fail with the error code E_RECOVER. If this
    /// occurs, the upper layer must complete the assisted recovery procedure
    /// and then try again.
    /// 
    /// This operation may be used as part of a recovery. If further errors
    /// occur during recovery, E_RECOVER is returned, and the procedure must
    /// be restarted.
    /// 
    pub fn journal_enqueue(&mut self, data: Option<&[u8]>, meta: Option<&[u8]>) -> Result<(), DharaError> {

        for _ in 0..DHARA_MAX_RETRIES {
            // Only try to program if head preparation succeeds.
            match self.prepare_head() {
                Ok(_) => {
                    // Only try to program if there is data.
                    match data {
                        Some(data) => {
                            match self.nand.prog(self.head, data){
                                Ok(_) => {return self.push_meta(meta);},
                                Err(e) => {self.recover_from(e)?;},
                            }
                        },
                        None => {
                            // We want to push meta anyway even if there is no data.
                            return self.push_meta(meta);
                        }
                    }
                },
                Err(e) => {self.recover_from(e)?;},
            }
        }
        Err(DharaError::TooBad)
    }

    /// Copy an existing page to the front of the journal. New metadata must
    /// be specified. This operation is not persistent until a checkpoint is
    /// reached.
    /// 
    /// This operation may fail with the error code E_RECOVER. If this
    /// occurs, the upper layer must complete the assisted recovery procedure
    /// and then try again.
    /// 
    /// This operation may be used as part of a recovery. If further errors
    /// occur during recovery, E_RECOVER is returned, and the procedure must
    /// be restarted.
    /// 
    pub fn journal_copy(&mut self, page: DharaPage, meta: Option<&[u8]>) -> Result<(),DharaError> {
        // TODO: use this logic like in dump_meta, or use match statements
        // and put the self.recover_from() in both the Err(e) branches?
        // let mut my_err: Result<u8,DharaError> = Ok(0);
        let mut my_err: Result<(),DharaError>; // Always gets assigned in the loop.

        for _ in 0..DHARA_MAX_RETRIES {
            my_err = self.prepare_head();
            if my_err.is_ok() {
                my_err = self.nand.copy(page, self.head);
                if my_err.is_ok() {
                    return self.push_meta(meta);
                }
            }
            // my_err should always be an error if we get here so unwrap_err() shouldn't panic.
            // Try to recover and eitehr exit with an error code or keep going around the loop.
            self.recover_from(my_err.unwrap_err())?;
        }
        Err(DharaError::TooBad)
    }

    /// Mark the journal dirty.
    pub fn journal_mark_dirty(&mut self) -> () {
        self.flags |= DHARA_JOURNAL_F_DIRTY;
    }

    /// Is the journal checkpointed? If true, then all pages enqueued are now
    /// persistent.
    pub fn journal_is_clean(&self) -> bool {
        self.flags & DHARA_JOURNAL_F_DIRTY == 0
    }

    /// True if journal is in recovery.
    pub fn journal_in_recovery(&self) -> bool {
        self.flags & DHARA_JOURNAL_F_RECOVERY != 0
    }

    /// If an operation returns E_RECOVER, you must begin the recovery
    /// procedure. You must then:
    /// 
    ///    - call dhara_journal_next_recoverable() to obtain the next block
    ///      to be recovered (if any). If there are no blocks remaining to be
    ///      recovered, DHARA_JOURNAL_PAGE_NONE is returned.
    /// 
    ///    - proceed to the next checkpoint. Once the journal is clean,
    ///      recovery will finish automatically.
    /// 
    /// If any operation during recovery fails due to a bad block, E_RECOVER
    /// is returned again, and recovery restarts. Do not add new data to the
    /// journal (rewrites of recovered data are fine) until recovery is
    /// complete.
    pub fn journal_next_recoverable(&mut self) -> DharaPage {
        let n = self.recover_next;

        if !self.journal_in_recovery() {
            return DHARA_PAGE_NONE;
        }

        if (self.flags & DHARA_JOURNAL_F_ENUM_DONE) != 0 {
            return DHARA_PAGE_NONE;
        }

        if self.recover_next == self.recover_root {
            self.flags |= DHARA_JOURNAL_F_ENUM_DONE;
        } else {
            self.recover_next = self.next_upage(self.recover_next);
        }

        return n;
    }

    // Some more getters, mostly for testing
    pub fn get_log2_ppc(&self) -> u8 {self.log2_ppc}
    pub fn get_head(&self) -> u32 {self.head}
    pub fn get_tail(&self) -> u32 {self.tail}
    pub fn get_tail_sync(&self) -> u32 {self.tail_sync}
    pub fn get_bb_current(&self) -> u32 {self.bb_current}
    pub fn get_bb_last(&self) -> u32 {self.bb_last}
    // TODO: get_root and journal_root do the same thing.  Eliminate one.
    pub fn get_root(&self) -> u32 {self.root}
    pub fn get_log2_ppb(&self) -> u8 {self.nand.get_log2_ppb()}
    pub fn get_num_blocks(&self) -> u32 {self.nand.get_num_blocks()}
    // And setters
    pub fn set_tail_sync(&mut self, v: u32) -> () {self.tail_sync = v;}
    
    // These functions are only used when simulating the nand.
    // #[cfg(test)]
    // pub fn freeze_stats(&mut self) -> () {
    //     self.nand.freeze();
    // }
    // #[cfg(test)]
    // pub fn thaw_stats(&mut self) -> () {
    //     self.nand.thaw();
    // }
}

// ///////////////////////////////////////////////////////////////////////
// Private methods
// ///////////////////////////////////////////////////////////////////////
//
impl<const N: usize,T: DharaNand> DharaJournal<N,T> {
    // TODO: A lot of these were marked as "inline" in the C code.
    // Leaving without that annotation for now, and we'll check results later.

    // ********************************************************************
    // Metapage binary format helpers

    // Note that every instance where hdr_*(*buf,...) is called in the C code
    // it is passing j->page_buf (the _start_ of the buffer, not somewhere
    // in the middle).  We can remove the function parameter, since these methods
    // have access to the buffer and never need to have a pointer to the middle.

    // Does the page buffer contain a valid checkpoint page?
    fn hdr_has_magic(&self) -> bool {
        (self.page_buf[0] == b'D')
            && (self.page_buf[1] == b'h')
            && (self.page_buf[2] == b'a')
    }

    // Insert the magic characters into the buffer.
    fn hdr_put_magic(&mut self) -> () {
        self.page_buf[0] = b'D';
        self.page_buf[1] = b'h';
        self.page_buf[2] = b'a';
    }

    // What epoch is this page?
    fn hdr_get_epoch(&self) -> u8 {
        self.page_buf[DHARA_HEADER_EPOCH_IDX]
    }

    // Set the epoch.
    fn hdr_set_epoch(&mut self, e: u8) -> () {
        self.page_buf[DHARA_HEADER_EPOCH_IDX] = e;
    }

    // Get the tail value in the page buffer.
    fn hdr_get_tail(&self) -> DharaPage {
        dhara_r32(&self.page_buf[DHARA_HEADER_TAIL_IDX..DHARA_HEADER_BBC_IDX])
    }

    // Set the tail.
    fn hdr_set_tail(&mut self, tail: DharaPage) -> () {
        dhara_w32(&mut self.page_buf[DHARA_HEADER_TAIL_IDX..DHARA_HEADER_BBC_IDX], tail)
    }

    fn hdr_get_bb_current(&self) -> DharaPage {
        dhara_r32(&self.page_buf[DHARA_HEADER_BBC_IDX..DHARA_HEADER_BBL_IDX])
    }

    fn hdr_set_bb_current(&mut self, bbc: DharaPage) -> () {
        dhara_w32(&mut self.page_buf[DHARA_HEADER_BBC_IDX..DHARA_HEADER_BBL_IDX], bbc)
    }

    fn hdr_get_bb_last(&self) -> DharaPage {
        dhara_r32(&self.page_buf[DHARA_HEADER_BBL_IDX..DHARA_HEADER_SIZE])
    }

    fn hdr_set_bb_last(&mut self, bbl: DharaPage) -> () {
        dhara_w32(&mut self.page_buf[DHARA_HEADER_BBL_IDX..DHARA_HEADER_SIZE], bbl)
    }

    // TODO: In the C code, this is only ever called with the NAND's 
    // log2 page size. For now, I've retained the size, but we could probably remove it.
    fn hdr_clear_user(&mut self, log2_page_size: usize) -> () {
        let start = DHARA_HEADER_SIZE + DHARA_COOKIE_SIZE;
        let end = 1 << log2_page_size;
        self.page_buf[start..end].fill(0xFF);
    }

    fn hdr_user_offset(&self, which: u32) -> usize {
        DHARA_HEADER_SIZE + DHARA_COOKIE_SIZE + (which as usize) * DHARA_META_SIZE
    }

    // ********************************************************************
    // Page geometry helpers on the struct

    // What is the successor of this block?
    fn next_block(&self, blk: DharaBlock) -> DharaBlock {
        let mut block = blk + 1;
        if block >= self.nand.get_num_blocks() {
            block = 0;
        }
        block
    }

    fn skip_block(&mut self) -> Result<u8,DharaError> {
        let next = self.next_block(self.head >> self.nand.get_log2_ppb());

        // We can't roll onto the same block as the tail.
        if self.tail_sync >> self.nand.get_log2_ppb() == next {
            return Err(DharaError::JournalFull);
        }

        self.head = next << self.nand.get_log2_ppb();
        if self.head == 0 {
            self.roll_stats();
        }
        Ok(0)
    }

    fn next_upage(&self, page: DharaPage) -> DharaPage {
        let mut p = page + 1;

        if is_aligned(p + 1, self.log2_ppc) {
            p += 1;
        }

        if p >= (self.nand.get_num_blocks() << self.nand.get_log2_ppb()) {
            p = 0;
        }
        p
    }

    // ********************************************************************
    // Journal setup/resume helpers

    fn clear_recovery(&mut self) -> () {
        self.recover_next = DHARA_PAGE_NONE;
        self.recover_root = DHARA_PAGE_NONE;
        self.recover_meta = DHARA_PAGE_NONE;
        self.flags &=  !(DHARA_JOURNAL_F_BAD_META |
            DHARA_JOURNAL_F_RECOVERY |
            DHARA_JOURNAL_F_ENUM_DONE);
    }

    fn reset_journal(&mut self) -> () {
        // We don't yet have a bad block estimate, so make
        // a conservative guess.
        self.epoch = 0;
        self.bb_last = self.nand.get_num_blocks() >> 6; // TODO: why?
        self.bb_current = 0;
        self.flags = 0;
        // Empty journal
        self.head = 0;
        self.tail = 0;
        self.tail_sync = 0;
        self.root = DHARA_PAGE_NONE;

        // No recovery required.
        self.clear_recovery();

        // Empty metadata buffer.
        self.page_buf.fill(0xFF);
    }

    fn roll_stats(&mut self) -> () {
        self.bb_last = self.bb_current;
        self.bb_current = 0;
        self.epoch += 1;
    }

    // Find the first checkpoint-containing block. If a block contains any
    // checkpoints at all, then it must contain one in the first checkpoint
    // location -- otherwise, we would have considered the block eraseable.
    //
    fn find_checkblock(&mut self, block: DharaBlock) -> Result<DharaBlock,DharaError> {
        let mut i: u8 = 0;
        let mut blk = block;

        while blk < self.nand.get_num_blocks() && i < DHARA_MAX_RETRIES {
            let p: DharaPage = (blk << self.nand.get_log2_ppb())
                | ((1 << self.log2_ppc) - 1);

            // The C code had one if() condition, and relied on 
            // the execution order of the conditions (read first, then 
            // has_magic() used the read.)
            // We're going to read and handle the Result differently.
            if !self.nand.is_bad(blk) {
                let res = self.nand.read(p, 0, 1 << self.nand.get_log2_page_size(), &mut self.page_buf);
                match res {
                    Err(_e) => (),
                    Ok(_) => if self.hdr_has_magic() {return Ok(blk);}
                }
            }
            blk += 1;
            i += 1;
        }

        // If we get this far, we haven't found one.
        Err(DharaError::TooBad)
    }

    // Perform a binary search for the last checkblock, starting
    // at "first".
    // Returns the number of the checkblock.
    fn find_last_checkblock(&mut self, first: DharaBlock) -> DharaBlock {
        let mut low = first;
        let mut high = self.nand.get_num_blocks() - 1;

        while low <= high {
            let mid = (low + high) >> 1;

            // This loads data into the page buffer in the process.
            let found = self.find_checkblock(mid);
            // Reads the page buffer changed in the previous statement.
            let different_epochs = self.hdr_get_epoch() != self.epoch;

            if found.is_err() || different_epochs {
                if mid == 0 {
                    return first;
                } else {
                    high = mid - 1;
                }
            } else {
                // If we get here, found can't be an error, so avoid the 
                // panic-handling requirements introduced by expect() or unwrap().
                let found: u32 = found.unwrap_or(0);
                if found + 1 >= self.nand.get_num_blocks() {
                    return found;
                }
                let nf = self.find_checkblock(found + 1);

                // Again, when using hdr_get_epoch(), we're relying on the
                // previous statement changing self.page_buf.
                if self.hdr_get_epoch() != self.epoch {
                    return found;
                }
                match nf {
                    Err(_) => {return found},
                    Ok(nf) => {low = nf;}
                }
            }
        }
        return first;
    }

    // Test whether a checkpoint group is in a state fit for reprogramming,
    // but allow for the fact that is_free() might not have any way of
    // distinguishing between an unprogrammed page, and a page programmed
    // with all-0xff bytes (but if so, it must be ok to reprogram such a
    // page).
    //
    // Formerly, the C version tested for an unprogrammed checkpoint group 
    // by checking to see if the first user-page had been programmed since 
    // last erase (by testing only the first page with is_free). This works 
    // if is_free is precise, because the pages are written in order.
    //
    // If is_free is imprecise, we need to check all pages in the group.
    // That also works, because the final page in a checkpoint group is
    // guaranteed to contain non-0xff bytes. Therefore, we return 1 only if
    // the group is truly unprogrammed, or if it was partially programmed
    // with some all-0xff user pages (which changes nothing for us).
    //
    fn cp_free(&mut self, first_user: DharaPage) -> bool {
        let count: usize = 1 << self.log2_ppc;

        for _ in 0..count {
            if !self.nand.is_free(first_user + 1) {
                return false;
            }
        }
        true
    }

    // Find the last checkpoint group in an erase block.
    // If a checkpoint group is completely unprogrammed, everything
	// following it will be completely unprogrammed also.
	// Therefore, binary search checkpoint groups until we find the
	// last programmed one.
    // block is the erase block number.
    // Returns the page number.
    fn find_last_group(&mut self, block: DharaBlock) -> DharaPage {
        let num_groups: u32 = 1 << (self.nand.get_log2_ppb() - self.log2_ppc);
        let mut low = 0;
        let mut high = num_groups - 1;

        while low <= high {
            let mid = (low + high) >> 1;
            let page: DharaPage = (mid << self.log2_ppc) 
                | (block << self.nand.get_log2_ppb());
            if self.cp_free(page) {
                high = mid - 1;
            } else if ((mid + 1) >= num_groups) 
                || self.cp_free(page + (1 << self.log2_ppc)){
                return page;
            } else {
                low = mid + 1;
            }
        }
        block << self.nand.get_log2_ppb()
    }

    // Find the and set the root of the journal.
    // Side effect is to change the root field.
    fn find_root(&mut self, start: DharaPage) -> Result<(), DharaError> {
        let block: DharaBlock = start >> self.nand.get_log2_ppb();
        let mut i: u32 = (start & ((1 << self.nand.get_log2_ppb()) - 1)) >> self.log2_ppc;

        loop {
            let page: DharaPage = (block << self.nand.get_log2_ppb()) + 
                ((i + 1) << self.log2_ppc) - 1;
            // Read a page into the buffer, which is also used by subsequent
            // functions.
            let result = self.nand.read(page, 0, 1 << self.nand.get_log2_page_size(), &mut self.page_buf);
            if result.is_ok() && self.hdr_has_magic() 
                    && (self.hdr_get_epoch() == self.epoch) {
                self.root = page - 1; // Found the root.
                return Ok(());
            }

            if i == 0 {
                break;  // C code used a signed for i, but that seems like
                        // a pain to keep changing back and forth.
            } else {
                i -= 1;
            }
        }
        Err(DharaError::TooBad)
    }

    // Starting from the last good checkpoint, find either:
    //   (a) the next free user-page in the same block, or
    //   (b) the first page of the next block.
    //
    // The block we end up on might be bad, but that's OK --
    // we'll skip it when we go to prepare the next write.
    // Note that C code returned an int, but it is always zero, and no error code.
    fn find_head(&mut self, start: DharaPage) -> () {
        self.head = self.next_upage(start);
        if self.head == 0 {
            self.roll_stats();
        }

        loop {
            // How many free pages trail this checkpoint group?
            let ppc: u32 = 1 << self.log2_ppc;
            let mut n: u32 = 0; 

            let first: DharaPage = self.head & !((ppc - 1) as DharaPage);

            while n < ppc && self.nand.is_free(first + ppc - n - 1) {
                n += 1;
            }

            // If we have some, then we've found our next free user page.
            if n > 1 {
                self.head = first + ppc - n;
                break;
            }

            // Skip to the next checkpoint group.
            self.head = first + ppc;
            if self.head >= (self.nand.get_num_blocks() << self.nand.get_log2_ppb()) {
                self.head = 0;
                self.roll_stats();
            }

            // If we hit the end of the block, we're done.
            if is_aligned(self.head, self.nand.get_log2_ppb()) {
                // Make sure we don't chase over the tail.
                if align_eq(self.head, self.tail, self.nand.get_log2_ppb()) {
                    self.tail = self.next_block(self.tail >> self.nand.get_log2_ppb()) << self.nand.get_log2_ppb();
                }
                break;
            }
        }
    }

    // Make sure the head pointer is on a ready-to-program page.
    fn prepare_head(&mut self) -> Result<(),DharaError> {
        let next = self.next_upage(self.head);

        // We can't write if doing so would cause the head pointer to
        // roll onto the same block as the last-synched tail.
        if align_eq(next, self.tail_sync, self.nand.get_log2_ppb())
                && !align_eq(next, self.head, self.nand.get_log2_ppb()) {
            return Err(DharaError::JournalFull);
        }

        self.flags |= DHARA_JOURNAL_F_DIRTY;
        if !is_aligned(self.head, self.nand.get_log2_ppb()) {
            return Ok(());
        }

        for _ in 0..DHARA_MAX_RETRIES {
            let block: DharaBlock = self.head >> self.nand.get_log2_ppb();

            if !self.nand.is_bad(block) {
                return self.nand.erase(block);
            }

            self.bb_current += 1;
            self.skip_block()?; // Returning the error, ignoring the Ok() case.
        }

        return Err(DharaError::TooBad);
    }

    fn restart_recovery(&mut self, old_head: DharaPage) -> () {
        // Mark the current head bad immediately, unless we're also using
        // it to hold our dumped metadata (it will then be marked bad at 
        // the end of recovery).
        if self.recover_meta == DHARA_PAGE_NONE 
                || !align_eq(self.recover_meta, old_head, self.nand.get_log2_ppb()) {
            self.nand.mark_bad(old_head >> self.nand.get_log2_ppb());
        } else {
            self.flags |= DHARA_JOURNAL_F_BAD_META;
        }

        // Start recovery again. Reset the source enumeration to the 
        // start of the original bad block, and reset the destination 
        // enumeration to the newly found good block.
        self.flags &= !DHARA_JOURNAL_F_ENUM_DONE;
        self.recover_next = self.recover_root & !((1u32 << self.nand.get_log2_ppb()) - 1);
        self.root = self.recover_root;
    }

    fn dump_meta(&mut self) -> Result<(),DharaError> {
        // We've just begun recovery on a new erasable block, but we have 
        // buffered metadata from the failed block.
        let mut my_err: Result<(),DharaError> = Ok(());

        for _ in 0..DHARA_MAX_RETRIES {
            my_err = self.prepare_head();
            if my_err.is_ok() {
                my_err = self.nand.prog(self.head, &self.page_buf);
                if my_err.is_ok() {
                    self.recover_meta = self.head;
                    self.head = self.next_upage(self.head);
                    if self.head == 0 {
                        self.roll_stats();
                    }
                    // Using "into()" method of u8 rather than "as usize".
                    self.hdr_clear_user(self.nand.get_log2_page_size().into());
                    return Ok(());
                }
            }
            // Report fatal errors.
            match my_err {
                Err(DharaError::BadBlock) => (),
                _ => return my_err,
            }

            self.bb_current += 1;
            self.nand.mark_bad(self.head >> self.nand.get_log2_ppb());
            self.skip_block()?;
        }

        Err(DharaError::TooBad)
    }

    fn recover_from(&mut self, write_err: DharaError) -> Result<(),DharaError> {
        let old_head: DharaPage = self.head;

        match write_err {
            DharaError::BadBlock => (),
            _ => {return Err(write_err);},
        }

        // Advance to the next free page.
        self.bb_current += 1;
        self.skip_block()?;

        // Are we already in the middle of a recovery?
        if self.journal_in_recovery() {
            self.restart_recovery(old_head);
            return Err(DharaError::Recover);
        }

        // Were we block aligned? No recovery required!
        if is_aligned(old_head, self.nand.get_log2_ppb()) {
            self.nand.mark_bad(old_head >> self.nand.get_log2_ppb());
            return Ok(());
        }

        self.recover_root = self.root;
        self.recover_next = self.recover_root & !((1u32 << self.nand.get_log2_ppb()) - 1);

        // Are we holding buffered metadata?  Dump it first.
        if !is_aligned(old_head, self.log2_ppc) {
            self.dump_meta()?;
        }

        self.flags |= DHARA_JOURNAL_F_RECOVERY;
        Err(DharaError::Recover)
    }

    fn finish_recovery(&mut self) -> () {
        // We just recoverd the last page. Mark the recovered
        // block as bad.
        self.nand.mark_bad(self.recover_root >> self.nand.get_log2_ppb());
        
        // If we had to dump metadata, and page on which we
        // did this also went pad, mark it bad too.
        if (self.flags & DHARA_JOURNAL_F_BAD_META) != 0 {
            self.nand.mark_bad(self.recover_meta >> self.nand.get_log2_ppb());
        }

        // Was the tail on this page?  Skip it forward.
        self.clear_recovery();
    }

    // Adds metadata to the page buffer.
    // param meta: None for an empty page and thus empty metadata.
    //             Some(&[u8]) reference to a buffer length DHARA_META_SIZE. 
    fn push_meta(&mut self, meta: Option<&[u8]>) -> Result<(),DharaError> {
        let old_head = self.head;
        let offset: usize = self.hdr_user_offset(self.head & ((1 << self.log2_ppc) - 1));

        // We have just written a user page.  Add the metadata
        // to the buffer.
        match meta {
            Some(meta) => self.page_buf[offset..offset+DHARA_META_SIZE].copy_from_slice(meta),
            None => self.page_buf[offset..offset+DHARA_META_SIZE].fill(0xFF),
        }

        // Unless we've filled the buffer, don't do any I/O.
        if !is_aligned(self.head + 2, self.log2_ppc) {
            self.root = self.head;
            self.head += 1;
            return Ok(());
        }

        // We don't need to check for immediate recover, because that'll
        // never happen -- we're not block-aligned.
        self.hdr_put_magic();
        self.hdr_set_epoch(self.epoch);
        self.hdr_set_tail(self.tail);
        self.hdr_set_bb_current(self.bb_current);
        self.hdr_set_bb_last(self.bb_last);

        if let Err(e) = self.nand.prog(self.head + 1, &self.page_buf) {
            return self.recover_from(e);
        }

        self.flags &= !DHARA_JOURNAL_F_DIRTY;
        self.root = old_head;
        self.head = self.next_upage(self.head);

        if self.head == 0 {
            self.roll_stats();
        }

        if self.flags & DHARA_JOURNAL_F_ENUM_DONE != 0 {
            self.finish_recovery();
        }

        if self.flags & DHARA_JOURNAL_F_RECOVERY == 0 {
            self.tail_sync = self.tail;
        }

        Ok(())
    }

}

// ********************************************************************
// Page geometry helpers independent of the struct

// Is this page aligned to N bits?
fn is_aligned(p: DharaPage, n: u8) -> bool {
    p & ((1u32 << n) - 1) == 0
}

// Are these two pages from the same alignment group?
fn align_eq(a: DharaPage, b: DharaPage, n: u8) -> bool {
    (a ^ b) >> n == 0
}

fn wrap(a: DharaPage, b: DharaPage) -> DharaPage {
    if a >= b {
        a - b
    } else {
        a
    }
}

// Calculate a checkpoint period: the largest value of ppc such that
// (2**ppc - 1) metadata blocks can fit on a page with one journal header.
fn choose_ppc(log2_psize: u8, max: u8) -> u8 {
    let max_meta: usize = (1 << log2_psize)
        - DHARA_HEADER_SIZE - DHARA_COOKIE_SIZE;
    let mut total_meta: usize = DHARA_META_SIZE;
    let mut ppc: u8 = 1;

    while ppc < max {
        total_meta <<= 1;
        total_meta += DHARA_META_SIZE;

        if total_meta > max_meta {
            break;
        }
        ppc += 1;
    }
    ppc
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::nand::{DharaBlock, DharaNand, DharaPage};

    struct SimpleNand {}

    impl DharaNand for SimpleNand {
        // A simulated 64 kiB NAND
        fn get_log2_page_size(&self) -> u8 {9} // 512 bytes/page, enough for 3 metadata blocks
        fn get_log2_ppb(&self) -> u8 {3}// 8 pages per erase block
        fn get_num_blocks(&self) -> u32 {16} // 16 erase blocks, or 128 pages total
        fn is_bad(&mut self, _blk: DharaBlock) -> bool {false}
        fn is_free(&mut self, _page: DharaPage) -> bool {true}
        fn mark_bad(&mut self, _blk: DharaBlock) -> () {()}
        fn read(&mut self, _page: u32, _offset: usize, _length: usize, data: &mut[u8]) -> Result<(), DharaError> {
            data.fill(0x55);
            Ok(())
        }
        fn erase(&mut self, _blk: DharaBlock) -> Result<(),DharaError> {Ok(())}
        fn copy(&mut self, _src: DharaPage, _dst: DharaPage) -> Result<(),DharaError> {Ok(())}
        fn prog(&mut self, _page: DharaPage, _data: &[u8]) -> Result<(),DharaError> {Ok(())}
        // Only used when simulating.
        // #[cfg(test)]
        // fn freeze(&mut self) -> () {()}
        // #[cfg(test)]
        // fn thaw(&mut self) -> () {()}
    }

    fn make_journal() -> DharaJournal::<512, SimpleNand> {
        let nand: SimpleNand = SimpleNand{};
        let buf: [u8; 512] = [0u8; 512]; // We start it with 0, but it gets changed to 0xFF when initialized.
        DharaJournal::<512, SimpleNand>::new(nand, buf)
    }

    #[test]
    fn test_header() -> () {
        // A bunch of trivial tests to make sure header get/set work correctly.
        let mut j = make_journal();

        // Magic values
        assert!(!j.hdr_has_magic());
        j.hdr_put_magic();
        assert!(j.hdr_has_magic());

        // Epoch
        assert_eq!(j.hdr_get_epoch(), 0xFF); // Whole buffer set to 0xFF by reset_journal().
        j.hdr_set_epoch(1);
        assert_eq!(j.hdr_get_epoch(), 1u8);

        // Tail
        assert_eq!(j.hdr_get_tail(), 0xFFFFFFFF);
        j.hdr_set_tail(0x0056AB1F);
        assert_eq!(j.hdr_get_tail(), 0x0056AB1F);

        // bb_current
        assert_eq!(j.hdr_get_bb_current(), 0xFFFFFFFF);
        j.hdr_set_bb_current(0x3578AF41);
        assert_eq!(j.hdr_get_bb_current(), 0x3578AF41);

        // bb_last
        assert_eq!(j.hdr_get_bb_last(), 0xFFFFFFFF);
        j.hdr_set_bb_last(0xAA558920);
        assert_eq!(j.hdr_get_bb_last(), 0xAA558920);

        // clear user
        // TODO: is there a way we can test clear_user()?

        // hdr_usr_offset
        assert_eq!(j.hdr_user_offset(2), 16+4+2*132);
    }

    #[test]
    #[should_panic]
    fn clear_too_much() -> () {
        let mut j = make_journal();
        j.hdr_clear_user(10);  // Clears 1024 bytes rather than 512.
    }

    #[test]
    fn page_geometry() -> () {
        // Tests unrelated to a journal.
        assert!(is_aligned(128, 6));
        assert!(!is_aligned(129, 6));
        assert!(align_eq(17, 18, 2)); // Same group of 2^2 = 4 pages.
        assert!(!align_eq(27, 18, 2));// Not in the same 4 pages.
        assert_eq!(wrap(7, 3), 4);
        assert_eq!(wrap(3, 7), 3);
        assert_eq!(choose_ppc(11, 6), 4); // Values for stationary logger.
        assert_eq!(choose_ppc(9, 3), 2); // Values for SimpleNand.

        // Tests of geometry methods.
        let j = make_journal();
        assert_eq!(j.next_block(0), 1);
        assert_eq!(j.next_block(15), 0); // 15 blocks.
        assert_eq!(j.log2_ppc, 2);
        assert_eq!(j.next_upage(0), 1);
        assert_eq!(j.next_upage(14), 16); // 15 user pages, then journal, so next is #16.
    }

}
