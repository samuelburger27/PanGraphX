pub mod core;
pub mod error;
pub mod formats;
pub mod traits;
pub enum GraphFormat {
    GFA_V1,
    GFA_V2,
    VG,
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
