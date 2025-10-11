# Claude Development Context

## Project Overview
This is a Rust port of the "Dhara" NAND flash translation layer https://github.com/dlbeer/dhara.

## Architecture
- The interface to higher level code is the DharaMap struct.
- File nand.rs contains a trait that must be implemented by the NAND flash driver.
- This library is intended to be used in no-std and no allocation environments like on microcontrollers.  Do not use std or Rust features that require an allocator.

## Development Commands
- Build: `cargo build`
- Test business logic: `cargo test`

## Repository Standards
- Prefer comments that explain why rather than say what the code does.

