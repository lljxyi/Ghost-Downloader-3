//! Bilibili 解析层的桥（`GD3_RUST_BILI_PARSE` 影子模式）：
//! `bili_pack.stream` 的 `parseDash` / `buildFnval` 交给 `gd3_net::bili`。
//! 配置值（alternativeQuality、HDR/Dolby 开关）由 Python 传入。

use gd3_net::bili;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pythonize::depythonize;

fn error(message: String) -> PyErr {
    PyValueError::new_err(message)
}

/// 镜像 `parseDash`：返回 `(videoUrl, audioUrl, videoSize, audioSize)`。
/// 非 0 出口（videoStreams 空 / matchStream 失败）抛 `ValueError`，文案与 Python 一致。
#[pyfunction]
#[pyo3(signature = (dash, duration, qn, accept_quality, audio_qn, alternative))]
pub fn bili_parse_dash(
    dash: &Bound<'_, PyAny>,
    duration: i64,
    qn: i64,
    accept_quality: Vec<i64>,
    audio_qn: i64,
    alternative: &str,
) -> PyResult<(String, String, i64, i64)> {
    let value: serde_json::Value = depythonize(dash)?;
    let outcome = bili::parse_dash(&value, duration, qn, &accept_quality, audio_qn, alternative)
        .map_err(error)?;
    Ok((
        outcome.video_url,
        outcome.audio_url,
        outcome.video_size,
        outcome.audio_size,
    ))
}

/// 镜像 `buildFnval(qn)`。
#[pyfunction]
#[pyo3(signature = (qn, include_hdr, include_dolby))]
pub fn bili_build_fnval(qn: i64, include_hdr: bool, include_dolby: bool) -> i64 {
    bili::build_fnval(qn, include_hdr, include_dolby)
}

/// `parseBiliUrl` 的桥（`GD3_RUST_BILI_URL=1` 影子模式）。
/// 成功 → 与 `BiliUrl` 字段同构的 dict；失败 → `ValueError`（文案与 Python 逐字一致）。
#[pyfunction]
pub fn bili_parse_url(py: Python<'_>, url: String) -> PyResult<Py<PyAny>> {
    let dict = PyDict::new(py);
    match gd3_net::bili::parse_url(&url) {
        Ok(outcome) => {
            dict.set_item("videoId", outcome.video_id)?;
            dict.set_item("mid", outcome.mid)?;
            dict.set_item("seasonId", outcome.season_id)?;
            dict.set_item("selectedPages", outcome.selected_pages)?;
            Ok(dict.into_any().unbind())
        }
        Err(message) => Err(PyValueError::new_err(message)),
    }
}

/// `buildPages` 的桥（`GD3_RUST_BILI_PAGES=1` 影子模式）。
/// 返回 dict：`{pages: [扁平 dict...], title, coverUrl}`；
/// `pages` 每个元素带 BiliPage 的全部构造参数（含 selected）。
/// KeyError("cid") 映射成 Python `KeyError`；int 失败映射 `ValueError`（文案一致）。
#[pyfunction]
#[pyo3(signature = (view_data, video_id, mid, season_id, selected_pages))]
pub fn bili_build_pages(
    py: Python<'_>,
    view_data: &Bound<'_, PyAny>,
    video_id: String,
    mid: i64,
    season_id: i64,
    selected_pages: Option<Vec<i64>>,
) -> PyResult<Py<PyAny>> {
    let value: serde_json::Value = depythonize(view_data)?;
    let list: Option<pyo3::Bound<'_, pyo3::types::PyList>> = match selected_pages {
        Some(pages) => Some(pyo3::types::PyList::new(py, pages)?),
        None => None,
    };
    let outcome = gd3_net::bili::build_pages(
        &value,
        &video_id,
        mid,
        season_id,
        list.as_deref()
            .map(|l| l.extract::<Vec<i64>>())
            .transpose()?
            .as_deref(),
    )
    .map_err(|error_value| match error_value {
        gd3_net::bili::BuildPagesError::KeyError(key) => pyo3::exceptions::PyKeyError::new_err(key),
        gd3_net::bili::BuildPagesError::PyValue(message) => PyValueError::new_err(message),
    })?;
    let dict = pyo3::types::PyDict::new(py);
    dict.set_item("title", outcome.title)?;
    dict.set_item("coverUrl", outcome.cover_url)?;
    let pages = pyo3::types::PyList::empty(py);
    for page in &outcome.pages {
        let page_dict = pyo3::types::PyDict::new(py);
        page_dict.set_item("index", page.index)?;
        page_dict.set_item("relativePath", "")?;
        page_dict.set_item("cid", page.cid)?;
        page_dict.set_item("pagePart", page.page_part.clone())?;
        page_dict.set_item("pageNumber", page.page_number)?;
        page_dict.set_item("bvid", page.bvid.clone())?;
        page_dict.set_item("episodeTitle", page.episode_title.clone())?;
        page_dict.set_item("sectionTitle", page.section_title.clone())?;
        page_dict.set_item("coverUrl", page.cover_url.clone())?;
        page_dict.set_item("_duration", page.duration)?;
        page_dict.set_item("selected", page.selected)?;
        pages.append(page_dict)?;
    }
    dict.set_item("pages", pages)?;
    Ok(dict.into_any().unbind())
}

/// `fetchView` 的桥（`GD3_RUST_BILI_VIEW` 影子模式）。
/// cookie 以键值对列表传（client 默认头），profile 决定 UA/指纹，
/// 与 `hf_fetch_tree` 同款限制。返回 view `data` dict。
#[pyfunction]
#[pyo3(signature = (video_id, mid, season_id, api_headers, client_profile))]
pub fn bili_fetch_view(
    py: Python<'_>,
    video_id: String,
    mid: i64,
    season_id: i64,
    api_headers: Vec<(String, String)>,
    client_profile: String,
) -> PyResult<Py<PyDict>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| PyValueError::new_err(error.to_string()))?;

    let client = gd3_net::build_client(None, None, &client_profile);
    let result = py.detach(|| {
        runtime.block_on(gd3_net::bili::fetch_view(
            &client,
            &api_headers,
            &video_id,
            mid,
            season_id,
        ))
    });
    let data = result.map_err(|error_value| match error_value {
        gd3_net::bili::ViewError::ValueError(message) => PyValueError::new_err(message),
        gd3_net::bili::ViewError::Transport(message) => PyValueError::new_err(message),
    })?;
    let value = pythonize::pythonize(py, &data)?;
    let dict = value.extract::<Bound<'_, PyDict>>()?;
    Ok(dict.clone().unbind())
}

/// `signParams` 的桥（`GD3_RUST_BILI_SIGN` 影子模式）：
/// 加 `wts`、按键排序、值清洗 `!'()*`、urlencode + md5 拼 `w_rid`。
#[pyfunction]
pub fn bili_sign_params(
    py: Python<'_>,
    params: &Bound<'_, PyDict>,
    mixin_key: String,
    wts: i64,
) -> PyResult<Py<PyDict>> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for (key, value) in params.iter() {
        let key: String = key.extract()?;
        let text = value.str()?;
        entries.push((key, text.to_str()?.to_string()));
    }
    // Python 是 `params["wts"] = round(time.time())`——赋值语义，调用方自带的 wts 被覆盖。
    entries.retain(|(key, _)| key != "wts");
    entries.push(("wts".to_string(), wts.to_string()));
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let cleaned: Vec<(String, String)> = entries
        .into_iter()
        .map(|(key, text)| {
            (
                key,
                text.chars().filter(|c| !"!'()*".contains(*c)).collect(),
            )
        })
        .collect();
    let rid = gd3_net::bili::wbi_rid(&cleaned, &mixin_key);
    let dict = pyo3::types::PyDict::new(py);
    for (key, text) in &cleaned {
        dict.set_item(key, text)?;
    }
    dict.set_item("w_rid", rid)?;
    Ok(dict.clone().unbind())
}

/// `signParams` 前置的派生（`_updateWbiKeys`）：两 URL 都有效才返回 Some；
/// key 合起来不足 64 字符时抛 `IndexError`（镜像 Python 的 `orig[i]`）。
#[pyfunction]
pub fn bili_mixin_key(img_url: String, sub_url: String) -> PyResult<Option<String>> {
    match gd3_net::bili::mixin_key_from_urls(&img_url, &sub_url) {
        Ok(key) => Ok(key),
        Err(_) => Err(pyo3::exceptions::PyIndexError::new_err(
            "string index out of range",
        )),
    }
}

/// `toCookie` 的桥（`GD3_RUST_BILI_SIGN` 影子模式）。
#[pyfunction]
pub fn bili_to_cookie(raw: String) -> String {
    gd3_net::bili::to_cookie(&raw)
}
