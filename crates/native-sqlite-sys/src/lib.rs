#![expect(non_snake_case, non_camel_case_types)]
#![cfg_attr(not(test), no_std)]

include!(concat!(env!("OUT_DIR"), "/upstream.rs"));
