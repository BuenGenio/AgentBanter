//! Templates come from a directory (hot-reloaded, so the look can change while the server
//! runs) or, when none is present, from the copies compiled into the binary.

use crate::assets;
use minijinja::{Environment, path_loader};
use minijinja_autoreload::AutoReloader;
use serde::Serialize;
use std::path::PathBuf;

pub struct Renderer {
    reloader: AutoReloader,
}

impl Renderer {
    pub fn new(dir: Option<PathBuf>) -> Self {
        let reloader = AutoReloader::new(move |notifier| {
            let mut env = Environment::new();
            match &dir {
                Some(dir) => {
                    env.set_loader(path_loader(dir));
                    notifier.watch_path(dir, true);
                }
                None => env.set_loader(|name| Ok(assets::template(name).map(str::to_string))),
            }
            Ok(env)
        });
        Self { reloader }
    }

    pub fn render<S: Serialize>(&self, name: &str, ctx: S) -> anyhow::Result<String> {
        let env = self.reloader.acquire_env()?;
        Ok(env.get_template(name)?.render(ctx)?)
    }
}
