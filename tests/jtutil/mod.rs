use dhara_rs::bytes::{dhara_r32, dhara_w32};
use dhara_rs::journal::{DharaJournal, DHARA_PAGE_NONE, DHARA_META_SIZE, DHARA_MAX_RETRIES};
// use dhara_rs::nand::DharaPage;
use dhara_rs::nand::{DharaNand, DharaPage};
use dhara_rs::DharaError;
use crate::sim::{seq_assert, seq_gen, SimNand, PAGE_SIZE};

/// To specify how many pages to enqueue.
pub enum Pages {
    All,
    Count(usize),
}

// Reduce typing for this specific test journal.
pub type SimJournal = DharaJournal::<512, SimNand>;

fn check_upage(j: &SimJournal, page: DharaPage) -> () {
    let mask: DharaPage = (1 << j.get_log2_ppc()) - 1;
    assert!((!page) & mask != 0);
    assert!(page < (j.get_num_blocks() << j.get_log2_ppb()));
}

pub fn jt_check(j: &SimJournal) -> () {
    // Head and tail pointers always point to a valid user-page
    // index (never a meta-page, and never out-of-bounds).
    check_upage(j, j.get_head());
    check_upage(j, j.get_tail());
    check_upage(j, j.get_tail_sync());

    // The head never advances forward onto the same block
    // as the tail.
    if (j.get_head() ^ j.get_tail_sync()) >> j.get_log2_ppb() == 0 {
		assert!(j.get_head() >= j.get_tail_sync());
	}

    // The current tail is always betwen the head and the
    // synchronized tail.  The C code relies on unsigned wrapping subtractions.
    assert!(j.get_head().wrapping_sub(j.get_tail_sync()) >= j.get_tail().wrapping_sub(j.get_tail_sync()));

    // The root always points to a valid user page in a non-empty
    // journal.
    if j.get_root() != DHARA_PAGE_NONE {
        let raw_size = j.get_head().wrapping_sub(j.get_tail());
        let root_offset = j.get_root().wrapping_sub(j.get_tail());

        check_upage(j, j.get_root());
        assert!(root_offset < raw_size);//
    }
}

fn recover(j: &mut SimJournal) -> () {
    let mut retry_count: usize = 0;
    let mut res: Result<u8, DharaError> = Ok(0);

    println!("    recover: start");

    while j.journal_in_recovery() {
        let page = j.journal_next_recoverable();

        jt_check(j);

        if page == DHARA_PAGE_NONE {
            res = j.journal_enqueue(None, None);
        } else {
            let mut meta = [0u8; DHARA_META_SIZE];
            j.journal_read_meta(page, &mut meta).expect("read_meta");
            res = j.journal_copy(page, Some(&meta));
        }

        jt_check(j);

        match res {
            Err(DharaError::Recover) => {
                println!("    recover: restart");
                retry_count += 1;
                if retry_count >= (DHARA_MAX_RETRIES as usize) {
                    panic!("recover with too many bad");
                }
                continue;
            },
            Err(e) => panic!("copy {:?}", e),
            Ok(_) => (),
        }
    }
    jt_check(j);
    println!("    recover: complete");
}

fn enqueue(j: &mut SimJournal, id: u32) -> Result<u8,DharaError> {
    let mut r: [u8; PAGE_SIZE] = [0u8; PAGE_SIZE];
    let mut meta: [u8; DHARA_META_SIZE] = [0u8; DHARA_META_SIZE];
    seq_gen(id as u64, &mut r); // Fill r with random data.
    dhara_w32(&mut meta[0..4], id);

    for _i in 0..DHARA_MAX_RETRIES {
        jt_check(j);
        match j.journal_enqueue(Some(&r), Some(&meta)) {
            Ok(_) => {return Ok(0);},
            Err(DharaError::Recover) => recover(j),
            Err(e) => {return Err(e);},
        }
    }
    Err(DharaError::TooBad)
}

// TODO: change count's type to a custom enum with variants Count(n) and All.
/// count: Count(number of pages to enqueue).  All => all pages in the NAND.
pub fn jt_enqueue_sequence(j: &mut SimJournal, start: usize, count: Pages) -> usize {
    let count:usize = match count {
        Pages::All => (j.get_num_blocks() << j.get_log2_ppb()) as usize,
        Pages::Count(count) => count,
    };

    for i in 0..count {
        let mut meta: [u8; DHARA_META_SIZE] = [0u8; DHARA_META_SIZE];

        match enqueue(j, (start+i) as u32) {
            Ok(_) => (),
            Err(DharaError::JournalFull) => {return i;},
            Err(e) => {panic!("enqueue {:?} i = {}", e, i);},
        }

        assert!(j.journal_size() >= i as u32);
        let root = j.journal_root();

        j.journal_read_meta(root, &mut meta).expect("read meta");
        assert_eq!(dhara_r32(&meta[0..4]), (start+i) as u32);
    }
    count
}

pub fn jt_dequeue_sequence(j: &mut SimJournal, next: usize, count: usize) -> () {
    // Shadow them to make them mutable.
    let mut count = count;
    let mut next = next;
    // To track garbage collection.
    let max_garbage: usize = 1 << j.get_log2_ppc();
    let mut garbage_count: usize = 0;

    while count > 0 {
        let mut meta: [u8; DHARA_META_SIZE] = [0u8; DHARA_META_SIZE];
        let tail = j.journal_peek();

        assert_ne!(tail, DHARA_PAGE_NONE);

        jt_check(j);
        j.journal_read_meta(tail, &mut meta).expect("read meta");

        jt_check(j);
        j.journal_dequeue();
        let id = dhara_r32(&meta[0..4]);

        if id == DHARA_PAGE_NONE {
            garbage_count += 1;
            assert!(garbage_count < max_garbage);
        } else {
            let mut r: [u8; PAGE_SIZE] = [0; PAGE_SIZE];

            assert_eq!(id as usize, next);
            garbage_count = 0;
            next += 1;
            count -= 1;
            j.nand.read(tail, 0, PAGE_SIZE, &mut r).expect("nand_read");

            seq_assert(id as u64, &r);
        }

        if count == 1 {
            println!("head={}, tail={}, root={}", j.get_head(), j.get_tail(), j.get_root());
        }

        jt_check(j);
    }

}