use dhara_rs::DharaError;
use dhara_rs::nand::{DharaBlock, DharaNand, DharaPage};

use rand::{Rng, RngCore, SeedableRng};
use rand::rngs::SmallRng;
use std::iter::zip;

pub const LOG2_PAGE_SIZE: u8 = 9;
pub const LOG2_PAGES_PER_BLOCK: u8 = 3;
const LOG2_BLOCK_SIZE: u8 = LOG2_PAGE_SIZE + LOG2_PAGES_PER_BLOCK;
pub const NUM_BLOCKS: usize =	113;

pub const PAGE_SIZE: usize      = 1 << LOG2_PAGE_SIZE; // 512 bytes, enough for 3 user metadata.
const PAGES_PER_BLOCK: usize = 1 << LOG2_PAGES_PER_BLOCK; // 8 pages/block
const BLOCK_SIZE: usize     = 1 << LOG2_BLOCK_SIZE; // 4096 bytes
const MEM_SIZE: usize       = NUM_BLOCKS * BLOCK_SIZE; // 4096 * 113 = 462_848 bytes

const BLOCK_BAD_MARK: u8 = 0x01;
const BLOCK_FAILED: u8   = 0x02;
const BLOCK_BOTH: u8 = BLOCK_FAILED | BLOCK_BAD_MARK;

// Struct used to capture call counts.
#[derive(Default)]
struct SimStats {
    frozen: bool,
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
    num_blocks: usize,
    // The simulated memory
    pages: Vec<u8>,
    //    pages: [u8; MEM_SIZE],
    // Keeps track of whether blocks are good.
    blocks: [BlockStatus; NUM_BLOCKS],
    // Keep track of statistics.
    stats: SimStats,
}

// Implementation of non-DharaNand methods.
impl SimNand {
    pub fn new() -> Self {
        let block = BlockStatus {flags: 0, next_page: PAGES_PER_BLOCK,
            timebomb: 0};
        let blocks = [block; NUM_BLOCKS];

        SimNand {
            log2_page_size: LOG2_PAGE_SIZE,
            log2_ppb: LOG2_PAGES_PER_BLOCK,
            num_blocks: NUM_BLOCKS,
            // The simulated memory
            pages: vec![0u8; MEM_SIZE],
            // pages: [0x55; MEM_SIZE],
            // Keeps track of whether blocks are good.
            blocks: blocks,
            // Keep track of statistics.
            stats: Default::default(),
        }
    }

    pub fn sim_reset(&mut self) -> () {
        self.stats = Default::default();
        self.pages.fill(0x55);
        for block in self.blocks.iter_mut() {
            block.flags = 0;
            block.next_page = PAGES_PER_BLOCK;
            block.timebomb = 0;
        }
    }

    pub fn timebomb_tick(&mut self, blkno: usize) -> () {
        if self.blocks[blkno].timebomb != 0 {
            self.blocks[blkno].timebomb -= 1;
            if self.blocks[blkno].timebomb == 0 {
                self.blocks[blkno].flags |= BLOCK_FAILED;
            }
        }
    }

    fn rep_status(&self, blkno: usize) -> char {
        match self.blocks[blkno].flags {
            BLOCK_FAILED => 'b',
            BLOCK_BAD_MARK => '?',
            BLOCK_BOTH => 'B',
            _ => if self.blocks[blkno].next_page != 0 {
                    ':'
                } else {
                    '.'
                },
        }
    }

    pub fn sim_set_failed(&mut self, blkno: usize) -> () {
        self.blocks[blkno].flags |= BLOCK_FAILED;
    }

    pub fn sim_set_timebomb(&mut self, blkno: usize, ttl: usize) -> () {
        self.blocks[blkno].timebomb = ttl;
    }

    pub fn sim_inject_bad(&mut self, count: usize) -> () {
        // Cache the generator for better loop performance.
        let mut rng = rand::thread_rng();

        for _i in 0..count {
            let blkno: usize = rng.gen::<usize>() % (NUM_BLOCKS);
            self.blocks[blkno].flags |= BLOCK_BOTH;
        }
    }

    pub fn sim_inject_failed(&mut self, count: usize) -> () {
        // Cache the generator for better loop performance.
        let mut rng = rand::thread_rng();

        for _i in 0..count {
            let blkno: usize = rng.gen::<usize>() % (NUM_BLOCKS);
            self.sim_set_failed(blkno);
        }
    }

    pub fn sim_inject_timebombs(&mut self, count: usize, max_ttl: usize) -> () {
        // Cache the generator for better loop performance.
        let mut rng = rand::thread_rng();

        for _i in 0..count {
            let blkno: usize = rng.gen::<usize>() % (NUM_BLOCKS);
            let ttl: usize = rng.gen::<usize>() % max_ttl + 1;
            self.sim_set_timebomb(blkno, ttl);
        }
    }

    pub fn sim_dump(&self) -> () {
        println!("NAND operation counts:");
        println!("    is_bad:         {}", self.stats.is_bad);
        println!("    mark_bad        {}", self.stats.mark_bad);
        println!("    erase:          {}", self.stats.erase);
        println!("    erase failures: {}", self.stats.erase_fail);
        println!("    is_erased:      {}", self.stats.is_erased);
        println!("    prog:           {}", self.stats.prog);
        println!("    prog failures:  {}", self.stats.prog_fail);
        println!("    read:           {}", self.stats.read);
        println!("    read (bytes):   {}", self.stats.read_bytes);
        println!("");
    
        println!("Block status:");
    
        let mut i: usize = 0;
        while i < NUM_BLOCKS {
            let mut j: usize = NUM_BLOCKS - i;
            if j > 64 {
                j = 64;
            }
            print!("    ");
            for k in 0..j {
                print!("{}", self.rep_status(i+k));
            }
            println!("");
            i += j;
        }
    }
    
    // Only used when simulating.
    // #[cfg(test)]
    fn freeze(&mut self) -> () {
        self.stats.frozen = true;
    }
    // #[cfg(test)]
    fn thaw(&mut self) -> () {
        self.stats.frozen = false;
    }
}

impl DharaNand for SimNand {
    fn get_log2_page_size(&self) -> u8 {self.log2_page_size}
    fn get_log2_ppb(&self) -> u8 {self.log2_ppb}
    fn get_num_blocks(&self) -> u32 {self.num_blocks as u32}

    fn is_bad(&mut self, blk: DharaBlock) -> bool {
        let block = blk as usize;
        assert!(block < NUM_BLOCKS, "sim: is_bad called on invalid block {blk}");
        if !self.stats.frozen {
            self.stats.is_bad += 1;
        }
        self.blocks[block].flags & BLOCK_BAD_MARK != 0
    }

    fn mark_bad(&mut self, blk: DharaBlock) -> () {
        let block = blk as usize;
        assert!(block < NUM_BLOCKS, "sim: mark_bad called on invalid block {blk}");
        if !self.stats.frozen {
            self.stats.mark_bad += 1;
        }
        self.blocks[block].flags |= BLOCK_BAD_MARK;
        ()
    }

    fn is_free(&mut self, page: DharaPage) -> bool {
        let blk: usize = (page >> LOG2_PAGES_PER_BLOCK) as usize;
        let pageno: u32 = page & ((1 << LOG2_PAGES_PER_BLOCK) - 1);
        assert!(blk < NUM_BLOCKS, "sim: is_free called on invalid block {blk}");

        if !self.stats.frozen {
            self.stats.is_erased += 1;
        }
        self.blocks[blk].next_page <= pageno as usize
    }

    fn erase(&mut self, blk: DharaBlock) -> Result<(),DharaError> {
        let block = blk as usize;
        assert!(block < NUM_BLOCKS, "sim: erase called on invalid block {blk}");
        assert!(self.blocks[block].flags & BLOCK_BAD_MARK == 0, "sim: erase 
            called on block which is marked bad: {block}");
        
        if !self.stats.frozen {
            self.stats.erase += 1;
        }

        // Remove the PAGES_PER_BLOCK indication of full.
        self.blocks[block].next_page = 0;

        self.timebomb_tick(block);

        let blk_idx: usize = block << LOG2_BLOCK_SIZE;

        if self.blocks[block].flags & BLOCK_FAILED != 0 {
            if !self.stats.frozen {
                self.stats.erase_fail += 1;
            }
            seq_gen((blk * 57 + 29) as u64, &mut self.pages[blk_idx..(blk_idx+BLOCK_SIZE)]);
            return Err(DharaError::BadBlock);
        }
        
        self.pages[blk_idx..(blk_idx + BLOCK_SIZE)].fill(0xFF);
        Ok(())
    }
    
    fn read(&mut self, page: u32, offset: usize, length: usize, data: &mut[u8]) -> Result<(), DharaError> {
        let blkno: usize = (page >> LOG2_PAGES_PER_BLOCK) as usize;
        let page_idx: usize = (page as usize) << LOG2_PAGE_SIZE;
        assert!(blkno < NUM_BLOCKS, "sim: prog called on invalid block {blkno}");

        let too_long = (offset > PAGE_SIZE) || (length > PAGE_SIZE) 
            || ((offset + length) > PAGE_SIZE);
        assert!(!too_long, "sim: read called on invalid range: offset = 
            {offset}, length = {length}");

        if !self.stats.frozen {
            self.stats.read += 1;
            self.stats.read_bytes += length;
        }

        let start: usize = page_idx + offset;
        let end: usize = start + length;
        data.copy_from_slice(&self.pages[start..end]);
        Ok(())
    }

    fn copy(&mut self, src: DharaPage, dst: DharaPage) -> Result<(),DharaError> {
        let mut buf: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
        self.read(src, 0, PAGE_SIZE, &mut buf)?;
        self.prog(dst, &buf)?;
        Ok(())
    }
    
    fn prog(&mut self, page: DharaPage, data: &[u8]) -> Result<(),DharaError> {
        let blkno: usize = (page >> LOG2_PAGES_PER_BLOCK) as usize;
        let pageno: usize = (page as usize) & ((1 << LOG2_PAGES_PER_BLOCK) - 1);
        let page_idx: usize = (page as usize) << LOG2_PAGE_SIZE;
        assert!(blkno < NUM_BLOCKS, "sim: prog called on invalid block {blkno}");
        assert!(self.blocks[blkno].flags & BLOCK_BAD_MARK == 0, "sim: prog 
            called on block which is marked bad: {blkno}");
        assert!(pageno >= self.blocks[blkno].next_page, "sim: prog \
            out-of-order page programming.  Block {blkno}, page {pageno} \
            (expected {})", self.blocks[blkno].next_page);

        if !self.stats.frozen {
            self.stats.prog += 1;
        }
        self.blocks[blkno].next_page = pageno + 1;
        self.timebomb_tick(blkno);

        if self.blocks[blkno].flags & BLOCK_FAILED != 0 {
            if !self.stats.frozen {
                self.stats.prog_fail += 1;
            }
            seq_gen((page * 57 + 29) as u64, &mut self.pages[page_idx..(page_idx+PAGE_SIZE)]);
            return Err(DharaError::BadBlock);
        }

        self.pages[page_idx..page_idx+PAGE_SIZE].copy_from_slice(data);
        Ok(())
    }

}

pub fn seq_gen(seed: u64, buf: &mut[u8]) -> () {
    let mut small_rng = SmallRng::seed_from_u64(seed);
    small_rng.fill_bytes(buf);

    // for &mut element in buf {
    //     element = small_rng.next_u8();
    // }

}

pub fn seq_assert(seed: u64, buf: &[u8]) -> () {
    let mut small_rng = SmallRng::seed_from_u64(seed);
    let length = buf.len();
    let mut expected = vec![0u8; length];
    // let mut expected: [u8; length] = [0, length];
    small_rng.fill_bytes(&mut expected[..]);

    for (&element, expect) in zip(buf, expected) {
        assert_eq!(element, expect, "seq_assert: mismatch in sequences.");
    }
}