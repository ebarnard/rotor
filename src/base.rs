pub trait Machine: Send + Sized {
    type Timeout;
}
