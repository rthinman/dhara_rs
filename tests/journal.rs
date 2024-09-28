mod jtutil;
mod sim;

use sim::SimNand;
use jtutil::{Pages, SimJournal, jt_enqueue_sequence, jt_dequeue_sequence};
use dhara_rs::journal::{DharaJournal, DHARA_PAGE_NONE};

fn suspend_resume(j: &mut SimJournal) -> () {
    let old_root = j.journal_root();
    let old_tail = j.get_tail();
    let old_head = j.get_head();

    j.journal_clear();
    assert_eq!(j.journal_root(), DHARA_PAGE_NONE);

    j.journal_resume().expect("resume"); // And panic/abort if there is an error.
    assert_eq!(old_root, j.journal_root());
    assert_eq!(old_tail, j.get_tail());
    assert_eq!(old_head, j.get_head());
}


fn dump_info(j: &SimJournal) -> () {
    println!("     log2_ppc  = {}", j.get_log2_ppc());  // TODO: create a getter, make the field public, define this as a test-only method on DharaJournal, or?
    println!("     size      = {}", j.journal_size());
    println!("     capacity  = {}", j.journal_capacity());
    println!("     bb_current= {}", j.get_bb_current());
    println!("     bb_last   = {}", j.get_bb_last());
}

#[test]
fn main_journal() -> () {
    // Set up the NAND first.
    let mut nand: SimNand = SimNand::new();
    nand.sim_reset();
    nand.sim_inject_bad(20);

    // Set up the journal's buffer.
    let buf: [u8; 512] = [0u8; 512]; // We start it with 0, but it gets changed to 0xFF when initialized.

    // Give them to the journal.
    let mut journal = SimJournal::new(nand, buf);
    let _ = journal.journal_resume(); // Ignore the result, even if an error.
    dump_info(&journal);

    println!("Enqueue/dequeue, 100 pages x 20");
    for _rep in 0..20 {
        let count = jt_enqueue_sequence(&mut journal, 0, Pages::Count(100));
        assert!(count == 100);
        print!("    size    = {} -> ", journal.journal_size());
        jt_dequeue_sequence(&mut journal, 0, count);
        println!("{}", journal.journal_size());
    }

    println!("Journal stats:");
    dump_info(&journal);
    println!("");

    println!("Enqueue/dequeue, ~100 pages x 20 (resume)");
    for rep in 0u32..20u32 {
        // let cookie = journal.get_cookie(); // TODO: C code gets a pointer to u8, not the actual cookie.
        // I didn't look to see where in the tests the cookie code was used.  Double check that this does
        // what we need.
        journal.set_cookie(rep);
        let mut count = jt_enqueue_sequence(&mut journal, 0, Pages::Count(100));
        assert_eq!(count, 100);

        while !journal.journal_is_clean() {
            let c = jt_enqueue_sequence(&mut journal, count, Pages::Count(1));
            count += 1;
            assert_eq!(c, 1);
        }

        print!("    size    = {} -> ", journal.journal_size());
        suspend_resume(&mut journal);
        jt_dequeue_sequence(&mut journal, 0, count);
        println!("{}", journal.journal_size());
        assert_eq!(journal.get_cookie(), rep);
    }
    println!("");

    println!("Journal stats:");
    dump_info(&journal);
    println!("");

    journal.nand.sim_dump(); // TODO: change if we make the nand field private again.
}