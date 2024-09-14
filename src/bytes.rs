// Helper functions to store multi-byte numbers
// independent of processor endianness.

// TODO: check if we need to declare these inline
// or whether the compiler does a good enough job.

// TODO: at the moment, these functions just take a
// slice and don't do any bounds checking, though at runtime
// the compiler should check that the underlying array
// is not accessed outside its bounds.  If we want
// to prevent panics, we my want to check length at
// compile time.  Some discussion in: 
// https://users.rust-lang.org/t/idiomatic-way-to-write-a-function-that-takes-a-slice-of-known-length/80142

pub fn dhara_r16(data: &[u8]) -> u16 {
    (data[0] as u16) | ((data[1] as u16) << 8)
}

pub fn dhara_w16(data: &mut [u8], v: u16) -> () {
    data[0] = v as u8;
    data[1] = (v >> 8) as u8;
}

pub fn dhara_r32(data: &[u8]) -> u32 {
    (data[0] as u32) 
    | ((data[1] as u32) << 8)
    | ((data[2] as u32) << 16) 
    | ((data[3] as u32) << 24)
}

pub fn dhara_w32(data: &mut [u8], v: u32) -> () {
    data[0] = v as u8;
    data[1] = (v >> 8) as u8;
    data[2] = (v >> 16) as u8;
    data[3] = (v >> 24) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_r16() {
        let a = [0x05u8, 0x06u8, 0x17u8, 0x03u8];
        let b = dhara_r16(&a[0..2]);
        assert_eq!(b, 0x0605);
    }

    #[test]
    fn check_w16() {
        let mut a = [0x05u8, 0x06u8, 0x17u8, 0x03u8];
        dhara_w16(&mut a[0..2], 0xAA55);
        assert_eq!(a, [0x55u8, 0xAAu8, 0x17u8, 0x03u8]);
    }

    #[test]
    fn check_r32() {
        let a = [0x05u8, 0x06u8, 0x17u8, 0x03u8];
        let b = dhara_r32(&a[..]);
        assert_eq!(b, 0x03170605);
    }

    #[test]
    fn check_w32() {
        let mut a = [0x05u8, 0x06u8, 0x17u8, 0x03u8];
        dhara_w32(&mut a[..], 0xAA550011);
        assert_eq!(a, [0x11u8, 0x00u8, 0x55u8, 0xAAu8]);
    }
    #[test]
    #[should_panic]
    fn access_beyond_end() {
        let a = [0x05u8, 0x06u8, 0x17u8, 0x03u8];
        let b = dhara_r32(&a[0..3]);
        assert_eq!(b, 0x0605);
    }
}
