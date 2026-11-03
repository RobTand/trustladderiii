use crate::{Error, Result};

#[derive(Debug, thiserror::Error)]
#[error("worker source {boundary} failed (errno {errno:?})", errno = self.raw_os_error())]
pub struct SourceError {
    pub boundary: &'static str,
    #[source]
    pub source: Box<Error>,
}

impl SourceError {
    pub fn raw_os_error(&self) -> Option<i32> {
        let mut cause: &(dyn std::error::Error + 'static) = self.source.as_ref();
        loop {
            if let Some(error) = cause.downcast_ref::<std::io::Error>() {
                return error.raw_os_error();
            }
            cause = cause.source()?;
        }
    }
}

pub(super) fn at<T, E: Into<Error>>(
    boundary: &'static str,
    result: std::result::Result<T, E>,
) -> Result<T> {
    #[cfg(test)]
    if let Some(errno) = FAILURE.with(|failure| {
        let mut failure = failure.borrow_mut();
        if failure.as_ref().is_some_and(|(name, _)| *name == boundary) {
            failure.take().map(|(_, errno)| errno)
        } else {
            None
        }
    }) {
        return Err(std::io::Error::from_raw_os_error(errno).into());
    }
    let _ = boundary;
    result.map_err(Into::into)
}

pub(super) fn cause(error: &Error) -> &Error {
    match error {
        Error::WorkerSource(context) => cause(&context.source),
        error => error,
    }
}

#[cfg(test)]
thread_local! {
    static FAILURE: std::cell::RefCell<Option<(&'static str, i32)>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(super) fn fail_next(boundary: &'static str, errno: i32) -> impl Drop {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAILURE.with(|failure| *failure.borrow_mut() = None);
        }
    }
    FAILURE.with(|failure| *failure.borrow_mut() = Some((boundary, errno)));
    Reset
}
