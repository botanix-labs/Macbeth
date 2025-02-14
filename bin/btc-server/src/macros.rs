#[macro_export]
macro_rules! badarg {
    ($($arg:tt)*) => {{
        tonic::Status::invalid_argument(format!($($arg)*))
    }};
}
