//! ftp URL 纯变换的桥（`GD3_RUST_FTP_URL=1` 影子模式）：
//! `features/ftp_pack/pack.py::parseFtpSource` 交给 `gd3_net::ftp::parse_ftp_source`。

use gd3_net::ftp;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// `parseFtpSource(url, defaultPort)` →
/// `(scheme, host, port, username, password, sourcePath, hasPort)`。
/// urlparse 抛 `ValueError` 的情形（Invalid IPv6 URL、bracketed host 校验、
/// port cast、`does not appear to be an IPv6 or IPv4 address` 等）直接抛
/// `PyValueError`，参数是 CPython 3.13 逐字报错文案。
#[pyfunction]
pub fn ftp_source(
    url: &str,
    default_port: i64,
) -> PyResult<(String, String, i64, String, String, String, bool)> {
    match ftp::parse_ftp_source(url, default_port) {
        Ok(shape) => Ok((
            shape.scheme,
            shape.host,
            shape.port,
            shape.username,
            shape.password,
            shape.source_path,
            shape.has_port,
        )),
        Err(message) => Err(PyValueError::new_err(message)),
    }
}
