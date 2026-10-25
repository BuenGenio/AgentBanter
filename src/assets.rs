//! Templates and static files compiled into the binary, used when no on-disk copy is found
//! (e.g. after `cargo install`). An on-disk `templates/` or `static/` directory always wins,
//! so the look can still be edited live.

pub const TEMPLATES: &[(&str, &str)] = &[
    ("capture.html", include_str!("../templates/capture.html")),
    (
        "partials/sidebar.html",
        include_str!("../templates/partials/sidebar.html"),
    ),
    ("base.html", include_str!("../templates/base.html")),
    ("index.html", include_str!("../templates/index.html")),
    ("session.html", include_str!("../templates/session.html")),
    (
        "partials/block.html",
        include_str!("../templates/partials/block.html"),
    ),
    (
        "partials/prompt.html",
        include_str!("../templates/partials/prompt.html"),
    ),
    (
        "partials/chips.html",
        include_str!("../templates/partials/chips.html"),
    ),
    (
        "partials/header_stats.html",
        include_str!("../templates/partials/header_stats.html"),
    ),
    (
        "partials/narrative.html",
        include_str!("../templates/partials/narrative.html"),
    ),
    (
        "partials/reasoning.html",
        include_str!("../templates/partials/reasoning.html"),
    ),
    (
        "partials/step.html",
        include_str!("../templates/partials/step.html"),
    ),
    (
        "partials/tool.html",
        include_str!("../templates/partials/tool.html"),
    ),
    (
        "partials/result.html",
        include_str!("../templates/partials/result.html"),
    ),
    (
        "partials/decision.html",
        include_str!("../templates/partials/decision.html"),
    ),
    (
        "partials/decision_panel.html",
        include_str!("../templates/partials/decision_panel.html"),
    ),
];

/// (path, content type, body)
pub const STATIC: &[(&str, &str, &str)] = &[
    (
        "monitor.js",
        "text/javascript; charset=utf-8",
        include_str!("../static/monitor.js"),
    ),
    (
        "decoder.js",
        "text/javascript; charset=utf-8",
        include_str!("../static/decoder.js"),
    ),
    (
        "vendor/acorn.js",
        "text/javascript; charset=utf-8",
        include_str!("../static/vendor/acorn.js"),
    ),
    (
        "vendor/acorn.LICENSE",
        "text/plain",
        include_str!("../static/vendor/acorn.LICENSE"),
    ),
    (
        "vendor/highlight.min.js",
        "text/javascript; charset=utf-8",
        include_str!("../static/vendor/highlight.min.js"),
    ),
    (
        "vendor/highlight.LICENSE",
        "text/plain",
        include_str!("../static/vendor/highlight.LICENSE"),
    ),
    (
        "inspector.js",
        "text/javascript; charset=utf-8",
        include_str!("../static/inspector.js"),
    ),
    (
        "capture.js",
        "text/javascript; charset=utf-8",
        include_str!("../static/capture.js"),
    ),
    (
        "sidebar.js",
        "text/javascript; charset=utf-8",
        include_str!("../static/sidebar.js"),
    ),
    (
        "app.css",
        "text/css; charset=utf-8",
        include_str!("../static/app.css"),
    ),
    (
        "app.js",
        "text/javascript; charset=utf-8",
        include_str!("../static/app.js"),
    ),
];

pub fn template(name: &str) -> Option<&'static str> {
    TEMPLATES.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

pub fn static_file(path: &str) -> Option<(&'static str, &'static str)> {
    STATIC
        .iter()
        .find(|(p, _, _)| *p == path)
        .map(|(_, ct, body)| (*ct, *body))
}
