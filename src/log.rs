#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        eprintln!("<3>{}", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        eprintln!("<4>{}", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        eprintln!("<6>{}", format_args!($($arg)*))
    };
}
