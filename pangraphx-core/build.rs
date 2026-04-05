fn main() {
    prost_build::compile_protos(&["proto/vg.proto"], &["proto/"]).unwrap();
}
