mod jtutil;
mod sim;

use sim::{SimJournal, SimNand};
use jtutil::{Pages, jt_enqueue_sequence, jt_dequeue_sequence};

/// Function to run all the scenarios.
/// Each scenario modifies the nand's block characteristics.
fn run(name: &str, scen: fn(&mut SimNand) -> ()) -> () {
    // Set up the NAND first.
    let mut nand: SimNand = SimNand::new();
    nand.sim_reset();

    scen(&mut nand);

    // Set up the journal's buffer.
    let buf: [u8; 512] = [0u8; 512]; // We start it with 0, but it gets changed to 0xFF when initialized.

    // Give them to the journal.
    let mut journal = SimJournal::new(nand, buf);

    // All tests are tuned for this value.
    assert_eq!(journal.get_log2_ppc(), 2);
    
    println!("========================================");
    println!("{}", name);
    println!("========================================");
    journal.nand.sim_dump();

    jt_enqueue_sequence(&mut journal, 0, Pages::Count(30));
    jt_dequeue_sequence(&mut journal, 0, 30);

    journal.nand.sim_dump();
}

fn scen_control(_n: &mut SimNand) -> () {

}

fn scen_instant_fail(n: &mut SimNand) -> () {
    n.sim_set_failed(0);
}

fn scen_after_check(n: &mut SimNand) -> () {
    n.sim_set_timebomb(0, 6);
}

fn scen_mid_check(n: &mut SimNand) -> () {
    n.sim_set_timebomb(0, 3);
}

fn scen_meta_check(n: &mut SimNand) -> () {
    n.sim_set_timebomb(0, 5);
}

fn scen_after_cascade(n: &mut SimNand) -> () {
    n.sim_set_timebomb(0, 6);
    n.sim_set_timebomb(1, 3);
    n.sim_set_timebomb(2, 3);
}

fn scen_mid_cascade(n: &mut SimNand) -> () {
    n.sim_set_timebomb(0, 3);
    n.sim_set_timebomb(1, 3);
}

fn scen_meta_fail(n: &mut SimNand) -> () {
    n.sim_set_timebomb(0, 3);
    n.sim_set_failed(1);
}

fn scen_bad_day(n: &mut SimNand) -> () {
    n.sim_set_timebomb(0, 7);

    for i in 1..5 {
        n.sim_set_timebomb(i, 3);
    }
}

#[test]
fn main_recovery() -> () {
    run("Control", scen_control);
    run("Instant fail", scen_instant_fail);

    run("Fail after checkpoint", scen_after_check);
    run("Fail mid-checkpoint", scen_mid_check);
    run("Fail on meta", scen_meta_check);

    run("Cascade fail after checkpoint", scen_after_cascade);
    run("Cascade fail mid-checkpoint", scen_mid_cascade);

    run("Metadata dump failure", scen_meta_fail);

    run("Bad day", scen_bad_day);
}