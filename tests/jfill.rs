mod jtutil;
mod sim;

use sim::SimNand;
use jtutil::{SimJournal, jt_enqueue_sequence, jt_dequeue_sequence};
use dhara_rs::journal::{DharaJournal, DHARA_PAGE_NONE};

fn fill() -> () {
    let mut nand: SimNand = SimNand::new();
    nand.sim_reset();
    // nand.sim_inject_bad(10);
    // nand.sim_inject_failed(10);

    // Set up the journal's buffer.
    let buf: [u8; 512] = [0u8; 512]; // We start it with 0, but it gets changed to 0xFF when initialized.

    // Give them to the journal.
    //println!("Journal init");
    let mut journal = SimJournal::new(nand, buf);
    //println!("    capacity: {}", journal.journal_capacity());
    //println!("");

    for rep in 0..5 {
        println!("Rep: {}", rep);

        println!("    enqueue until error...");
        // let count = jt_enqueue_sequence(&mut journal, 0, None);
        let count = jt_enqueue_sequence(&mut journal, 0, None);
        println!("    enqueue count: {}", count);
        println!("    size:          {}", journal.journal_size());

        println!("    dequeue...");
        jt_dequeue_sequence(&mut journal, 0, count);
        println!("    size:          {}", journal.journal_size());

        // Only way to recover space here...
        journal.set_tail_sync(journal.get_tail());
    }
    println!("");
}

#[test]
fn main_jfill() -> () {
    // TODO: put this in a loop and maybe pass a seed?
    fill();
}