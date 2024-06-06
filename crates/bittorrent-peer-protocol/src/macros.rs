macro_rules! check_length {
    ($v:expr, $e:expr) => {
        if $v < $e {
            return Err($crate::Error::BadLength($v, $e));
        }
    };
}
