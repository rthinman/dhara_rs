mod sim;

use dhara_rs::journal::{DHARA_META_SIZE, DHARA_PAGE_NONE};
use dhara_rs::nand::DharaPage;
use dhara_rs::{meta_get_alt, meta_get_id, DharaError, DharaMap, DharaSector};
use rand::{Rng, RngCore, SeedableRng};
use rand::rngs::SmallRng;
use sim::{seq_assert, seq_gen, SimNand, PAGE_SIZE};

// Reduce typing for this specific test map.
pub type SimMap = DharaMap::<512, SimNand>;

const NUM_SECTORS: usize = 200;
const GC_RATIO: u8 = 4;

struct SectorList {
    // rng: SmallRng,
    list: [DharaSector; NUM_SECTORS],
}

impl SectorList {
    pub fn new() -> Self {
        SectorList {
            list: [0; NUM_SECTORS],
        }
    }

    pub fn shuffle(&mut self, seed: u64) -> () {
        // Implemented similarly to the C code, but there
        // could be other ways to shuffle (with a crate).
        let mut small_rng = SmallRng::seed_from_u64(seed);

        for i in 0..NUM_SECTORS {
            self.list[i] = i.try_into().expect("failed to coerce");
        }

        // C code does not hit zero, hence the 1 below.
        for i in (1..NUM_SECTORS).rev() {
            let j = small_rng.gen::<usize>() % i;
            let tmp = self.list[i];

            self.list[i] = self.list[j];
            self.list[j] = tmp;
        }
    }

    // I could just make list public, but whatever.
    pub fn get(&self, idx: usize) -> DharaSector {
        self.list[idx]
    }
}

fn check_recurse(m: &mut SimMap, parent: DharaPage, page: DharaPage, id_expect: DharaSector, depth: usize) -> usize {
    let mut meta: [u8; DHARA_META_SIZE]= [0u8; DHARA_META_SIZE];
    let h_offset: DharaPage = m.journal.get_head() - m.journal.get_tail();
    let p_offset: DharaPage = parent - m.journal.get_tail();
    let offset: DharaPage = page - m.journal.get_tail();

    let mut count: usize = 1;

    if page == DHARA_PAGE_NONE {
        return 0;
    }

    // Make sure this is a valid journal user page, and one which is
    // older than the page pointing to it.
    assert!(offset < p_offset);
    assert!(offset < h_offset);
    assert!( (!page) & ((1 << m.journal.get_log2_ppc()) - 1) != 0 );

    // Fetch metadata.
    m.journal.journal_read_meta(page, &mut meta).expect("mt_check");

    // Check the first <depth> bits of the ID field.
    let id = meta_get_id(&meta);
    // TODO: double check this.  It looks to me like the original code if depth == 0 {id_expect = id} else...
    // doesn't do anything in the == 0 case because id_expect is not used after this point.  I changed it
    // to the below, only doing the other case.
    if depth != 0 {
        // assert!( !((id ^ id_expect) >> (32-depth)) );
        assert!( (id ^ id_expect) >> (32 - depth) == 0);
    }

    // Check all alt pointers.
    for i in depth..32 {
        let child: DharaPage = meta_get_alt(&meta, i);

        count += check_recurse(m, page, child, id ^ (1 << (31 - i)), i + 1);
    }

    return count;
}

fn mt_check(m: &mut SimMap) -> () {
    m.journal.nand.freeze();

    let count = check_recurse(m, m.journal.get_head(), m.journal.get_root(), 0, 0);

    m.journal.nand.thaw();
}

fn mt_write(m: &mut SimMap, s: DharaSector, seed: u64) -> () {
    let mut buf: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
    seq_gen(seed, &mut buf);
    m.write(s, &buf).expect("map_write");
}

fn mt_assert(m: &mut SimMap, s: DharaSector, seed: u64) -> () {
    let mut buf: [u8; PAGE_SIZE] = [0; PAGE_SIZE];
    m.read(s, &mut buf).expect("map_read");
    seq_assert(seed, &buf);
}

fn mt_trim(m: &mut SimMap, s: DharaSector) -> () {
    m.trim(s).expect("map_trim");
}

fn mt_assert_blank(m: &mut SimMap, s: DharaSector) -> () {
    match m.find(s) {
        Ok(loc) => {assert!(false, "find found a value {} when it should not have", loc);},
        Err(e) => {assert_eq!(e, DharaError::NotFound);}
    }
}

fn mt_test() -> () {
    // List of sectors for tests.
    let mut sector_list = SectorList::new();

    // Set up the NAND first.
    let mut nand: SimNand = SimNand::new();
    nand.sim_reset();
    nand.sim_inject_bad(10);
    nand.sim_inject_timebombs(30, 20);

    // Set up the journal's buffer.
    let buf: [u8; 512] = [0u8; 512]; // We start it with 0, but it gets changed to 0xFF when initialized.

    // Give them to the map.
    println!("Map init");
    let mut map = SimMap::new(nand, buf, GC_RATIO);
    map.resume(); // .expect("map resume failed");
    println!("  capacity: {}", map.get_capacity());
    println!("  sector count: {}", NUM_SECTORS);
    println!();

    println!("Sync...");
    map.sync();
    println!("Resume...");
    // map.init(); // Doesn't exist in Rust implementation. TODO: should it?
    map.resume(); // .expect("map resume failed");

    println!("Writing sectors...");
    sector_list.shuffle(0); //TODO: check these low bit seeds are OK.
    for i in 0..NUM_SECTORS {
        let s = sector_list.get(i);
        mt_write(&mut map, s, s as u64);
        mt_check(&mut map);
    }

    println!("Sync...");
    map.sync();
    println!("Resume...");
    // map.init(); // Doesn't exist in Rust implementation. TODO: should it?
    map.resume(); // .expect("map resume failed");
    println!("  capacity: {}", map.get_capacity());
    println!("  use count: {}", map.get_size());
    println!();

    println!("Read back...");
    sector_list.shuffle(1); //TODO: check these low bit seeds are OK.
    for i in 0..NUM_SECTORS {
        let s = sector_list.get(i);
        mt_assert(&mut map, s, s as u64);
    }

    println!("Rewrite/trim half...");
    sector_list.shuffle(2); //TODO: check these low bit seeds are OK.
    for i in (0..NUM_SECTORS).step_by(2) {
        let s0 = sector_list.get(i);
        let s1 = sector_list.get(i + 1);

        mt_write(&mut map, s0, !s0 as u64);
        mt_check(&mut map);
        mt_trim(&mut map, s1);
        mt_check(&mut map);
    }

    println!("Sync...");
    map.sync();
    println!("Resume...");
    // map.init(); // Doesn't exist in Rust implementation. TODO: should it?
    map.resume(); // .expect("map resume failed");
    println!("  capacity: {}", map.get_capacity());
    println!("  use count: {}", map.get_size());
    println!();

    println!("Read back...");
    for i in (0..NUM_SECTORS).step_by(2) {
        let s0 = sector_list.get(i);
        let s1 = sector_list.get(i + 1);

        mt_assert(&mut map, s0, !s0 as u64);
        mt_assert_blank(&mut map, s1);
    }
    println!("");
}

#[test]
fn main_map() -> () {
    for i in 0..1000 {
        // Each iteration should inject different bad blocks and timebombs.
        mt_test();
    }

    // This doesn't exactly recreate the C code, because there the sim 
    // statistics are cumulative over all the tests.
    // sim_dump();
}

