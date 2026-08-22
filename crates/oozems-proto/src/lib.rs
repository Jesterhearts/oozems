#![forbid(unsafe_code)]

pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/oozems.v1.rs"));
}

pub const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";
