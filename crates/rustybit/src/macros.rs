#[macro_export]
macro_rules! try_into {
    ($expr:expr, $target_type:ty) => {
        TryInto::<$target_type>::try_into($expr).map_err(|_| anyhow::anyhow!("Type casting error"))
    };
}
