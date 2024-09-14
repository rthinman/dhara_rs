use dhara_rs::journal::DharaJournal;
use dhara_rs::nand::DharaNand;

#[test]
fn create_journal () {
    // TODO: replace with something better.
    let nand = DharaNand {
        log2_page_size: 11,
        log2_ppb: 2,
        num_blocks: 6,
    }

    let mut j: DharaJournal<16> = DharaJournal::new(nand);
    
    j.set_ppc(7);

    assert_eq!(j.get_ppc(), 7);
}