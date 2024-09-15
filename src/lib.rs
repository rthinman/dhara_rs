mod bytes;
pub mod journal;
pub mod nand;

// Constants
const DHARA_SECTOR_NONE:u32 = 0xffffffff;  // TODO: if we have Option/Result return types, do we need this?

// TODO: possible move to a new module, to include human-readable functions.
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

// pub struct DharaMap<const N: usize> {
//     journal: journal::DharaJournal<N>,
//     gc_ratio: u8,
//     count: u32,  // TODO: change to a custom type like the typedef dhara_sector_t?
// }

// impl<const N: usize> DharaMap<N> {
//     // Renamed from the original "init" to match common Rust usage.
//     pub fn new(nand: nand::DharaNand, page_buf: [u8; N], gc_ratio: u8) -> Self {
//         // TODO: add driver to parameters, and decide how to handle the page buffer (declare a size here, or pass a buffer like in C? 
//         // kind of depends on how it is used)
//         // page buffer is used exclusively by the journal, but I'm guessing its size depends on the implementation.  
//         // Might be good to declare it externally and MOVE it first here and then to the journal.
//         let mut ratio: u8 = gc_ratio;
//         if ratio == 0 {
//             ratio = 1;
//         }

// //        let mut journal = journal::DharaJournal::<N>::new(nand, page_buf);
//         let mut journal = journal::DharaJournal::<N>::new();
        
//         DharaMap {
//             journal: journal,
//             gc_ratio: ratio,
//             count: 0, // TODO: This will get updated when resume() is called.
//         }
//     }

//     pub fn resume(&mut self) -> Result<(), DharaError> {
//         Ok(())
//     }

//     pub fn clear(&mut self) -> () {

//     }

//     pub fn get_capacity(&self) -> u32 {
//         // TODO: this could return the custom type.
//         5
//     }

//     pub fn get_size(&self) -> u32 {
//         // TODO: this could return the custom type.
//         5
//     }

//     pub fn find(&self) -> Result<u32, DharaError> {
//         // TODO: change u32 to dhara_page_t equiv?
//         Err(DharaError::NotFound)
//     }

//     pub fn read(&self, sector: u32, data: &mut [u8]) -> Result<(), DharaError> {
//         Ok(())
//     } 

//     pub fn write(&mut self, sector: u32, data: &[u8]) -> Result<(), DharaError> {
//         Ok(())
//     } 

//     pub fn copy_page(&mut self, src_sector: u32, dst_sector: u32) -> Result<(), DharaError> {
//         Ok(())
//     } 

//     pub fn trim(&mut self, sector: u32) -> Result<(), DharaError> {
//         Ok(())
//     } 

//     pub fn sync(&mut self) -> Result<(), DharaError> {
//         Ok(())
//     } 

//     pub fn gc(&mut self) -> Result<(), DharaError> {
//         Ok(())
//     } 

// }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
