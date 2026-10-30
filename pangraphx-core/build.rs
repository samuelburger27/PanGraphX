fn main() {
    prost_build::compile_protos(&["proto/vg.proto", "proto/vg_read.proto"], &["proto/"]).unwrap();
}
