mod build;
mod footnotes;
mod path_utils;
mod plan;
mod renderer_hooks;
mod toc;

pub(crate) use build::{build_multi_page_html, RenderedPage};
pub(crate) use plan::{build_multi_page_plan, MultiPagePlan};

#[cfg(test)]
mod tests;
