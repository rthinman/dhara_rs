use dhara_rs::DharaError;
use dhara_rs::nand::{DharaBlock, DharaNand, DharaPage};

const LOG2_PAGE_SIZE: u8 = 9;
const LOG2_PAGES_PER_BLOCK: u8 = 3;
const LOG2_BLOCK_SIZE: u8 = LOG2_PAGE_SIZE + LOG2_PAGES_PER_BLOCK;
const NUM_BLOCKS: u32 =	113;

const PAGE_SIZE: u32      = 1 << LOG2_PAGE_SIZE; // 512 bytes, enough for 3 user metadata.
const PAGES_PER_BLOCK: usize = 1 << LOG2_PAGES_PER_BLOCK; // 8 pages/block
const BLOCK_SIZE: u32     = 1 << LOG2_BLOCK_SIZE; // 4096 bytes
const MEM_SIZE: u32       = NUM_BLOCKS * BLOCK_SIZE; // 4096 * 113 = 462_848 bytes

const BLOCK_BAD_MARK: u8 = 0x01;
const BLOCK_FAILED: u8   = 0x02;

// Struct used to capture call counts.
#[derive(Default)]
struct SimStats {
    frozen: usize,
    is_bad: usize,
    mark_bad: usize,
    erase: usize,
    erase_fail: usize,
    is_erased: usize,
    prog: usize,
    prog_fail: usize,
    read: usize,
    read_bytes: usize,
}

// Struct to keep track of blocks.
#[derive(Clone, Copy)]
struct BlockStatus {
    flags: u8,
    // Index of the next unprogrammed page.  0 means a fully erased
    // block, and  PAGES_PER_BLOCK is a fully programmed block.
    next_page: usize,
    // Timebomb counter: if non-zero, this is the number of
    // operations until permanent failure.
    timebomb: usize,
}

pub struct SimNand {
    log2_page_size: u8,
    log2_ppb: u8,
    num_blocks: u32,
    // The simulated memory
    pages: [u8; MEM_SIZE as usize],
    // Keeps track of whether blocks are good.
    blocks: [BlockStatus; NUM_BLOCKS as usize],
    // Keep track of statistics.
    stats: SimStats,
}

// Implementation of non-DharaNand methods.
impl SimNand {
    pub fn new() -> Self {
        let block = BlockStatus {flags: 0, next_page: PAGES_PER_BLOCK,
            timebomb: 0};
        let blocks = [block; NUM_BLOCKS as usize];

        SimNand {
            log2_page_size: LOG2_PAGE_SIZE,
            log2_ppb: LOG2_PAGES_PER_BLOCK,
            num_blocks: NUM_BLOCKS,
            // The simulated memory
            pages: [0x55; MEM_SIZE as usize],
            // Keeps track of whether blocks are good.
            blocks: blocks,
            // Keep track of statistics.
            stats: Default::default(), //SimStats{}, // Default is derived as zero.
        }
    }

    pub fn reset(&mut self) -> () {
        self.stats = Default::default(); // Just create an empty one.
        self.pages.fill(0x55);
        for block in self.blocks.iter_mut() {
            block.flags = 0;
            block.next_page = PAGES_PER_BLOCK;
            block.timebomb = 0;
        }
    }

    pub fn timebomb_tick(&mut self, blkno: DharaBlock) -> () {
        let idx: usize = blkno as usize;

        if self.blocks[idx].timebomb != 0 {
            self.blocks[idx].timebomb -= 1;
            if self.blocks[idx].timebomb == 0 {
                self.blocks[idx].flags |= BLOCK_FAILED;
            }
        }
    }

}

impl DharaNand for SimNand {
    fn get_log2_page_size(&self) -> u8 {self.log2_page_size}
    fn get_log2_ppb(&self) -> u8 {self.log2_ppb}
    fn get_num_blocks(&self) -> u32 {self.num_blocks}

    fn is_bad(&self, blk: DharaBlock) -> bool {
        assert!(blk < NUM_BLOCKS, "sim: is_bad called on invalid block {blk}");
        if self.stats.frozen == 0 {
            self.stats.is_bad += 1;
        }
        self.blocks[blk as usize].flags & BLOCK_BAD_MARK == 0
    }

    fn mark_bad(&self, blk: DharaBlock) -> () {
        assert!(blk < NUM_BLOCKS, "sim: mark_bad called on invalid block {blk}");
        if self.stats.frozen == 0 {
            self.stats.mark_bad += 1;
        }
        self.blocks[blk as usize].flags |= BLOCK_BAD_MARK;
        ()
    }

    fn is_free(&self, page: DharaPage) -> bool {
        let blk: usize = page >> LOG2_PAGES_PER_BLOCK;
        let pageno: u32 = page & ((1 << LOG2_PAGES_PER_BLOCK) - 1);
        assert!(blk < NUM_BLOCKS, "sim: is_free called on invalid block {blk}");

        if self.stats.frozen == 0 {
            self.stats.is_erased += 1;
        }
        self.blocks[blk as usize].next_page <= pageno
    }

    fn erase(&self, blk: DharaBlock) -> Result<u8,DharaError> {
        assert!(blk < NUM_BLOCKS, "sim: erase called on invalid block {blk}");
        assert!(self.blocks[blk as usize].flags & BLOCK_BAD_MARK != 0, "sim: erase 
            called on block which is marked bad: {blk}");
        
        if self.stats.frozen == 0 {
            self.stats.erase += 1;
        }
        self.timebomb_tick(blk);

        if self.blocks[blk as usize].flags & BLOCK_FAILED != 0 {
            if self.stats.frozen == 0 {
                self.stats.erase_fail += 1;
            }
            // TODO: seq_gen(blk * 57 + 29, blk_idx, BLOCK_SIZE);
            return Err(DharaError::BadBlock);
        }
            
        Ok(0)
    }
    
    fn read(&self, _page: u32, _offset: usize, _length: usize, data: &mut[u8]) -> Result<u8, DharaError> {
        data.fill(0x55);
        Ok(0)
    }

    fn copy(&self, _src: DharaPage, _dst: DharaPage) -> Result<u8,DharaError> {Ok(0)}
    
    fn prog(&self, _page: DharaPage, _data: &[u8]) -> Result<u8,DharaError> {Ok(0)}

}