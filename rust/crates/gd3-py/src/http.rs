//! http_pack 判定层的桥（`GD3_RUST_HTTP_MATCH` 影子模式）：
//! `features/http_pack/pack.py::HttpParser.match` / `matchPassive` 交给
//! `gd3_core::http_pack`。纯函数，无网络，无状态。

use gd3_core::http_pack;
use pyo3::prelude::*;

#[pyfunction]
pub fn http_parser_match(url: String) -> bool {
    http_pack::parser_match(&url)
}

#[pyfunction]
pub fn http_parser_match_passive(url: String) -> bool {
    http_pack::parser_match_passive(&url)
}
