use anyhow::Result;
use rushdown::ast::{KindData, Link, LinkKind, NodeRef};

use crate::conditional::RenderProfile;
use crate::graph::PreparedDocument;

pub(crate) struct MarkdownRenderer {
    profile: RenderProfile,
}

struct LinkRewrite {
    doc_id: usize,
    node_ref: NodeRef,
    destination: String,
    is_image: bool,
}

impl MarkdownRenderer {
    pub(crate) fn new(profile: RenderProfile) -> Self {
        Self { profile }
    }

    pub(crate) fn render_document(&self, prepared: &mut PreparedDocument) -> Result<String> {
        rewrite_graph_links_in_place(prepared);
        crate::fmt::format_prepared_markdown(prepared, &self.profile)
    }
}

fn rewrite_graph_links_in_place(prepared: &mut PreparedDocument) {
    let rewrites = {
        let graph = prepared.graph();
        let mut rewrites = Vec::<LinkRewrite>::new();
        for doc_id in 0..graph.documents.len() {
            let doc = graph.document(doc_id);
            let mut nodes = Vec::new();
            collect_doc_nodes(&doc.arena, doc.doc_ref, &mut nodes);
            for node_ref in nodes {
                match doc.arena[node_ref].kind_data() {
                    KindData::Link(_) => {
                        if let Some(target) = graph.link_target(doc_id, node_ref) {
                            rewrites.push(LinkRewrite {
                                doc_id,
                                node_ref,
                                destination: rewrite_target_for_output(target),
                                is_image: false,
                            });
                        }
                    }
                    KindData::Image(_) => {
                        if let Some(target) = graph.image_target(doc_id, node_ref) {
                            rewrites.push(LinkRewrite {
                                doc_id,
                                node_ref,
                                destination: rewrite_target_for_output(target),
                                is_image: true,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        rewrites
    };

    let graph = prepared.graph_mut();
    for rewrite in rewrites {
        let doc = graph.document_mut(rewrite.doc_id);
        if rewrite.is_image {
            let Some(new_kind) = (match doc.arena[rewrite.node_ref].kind_data() {
                KindData::Image(image) => Some(if let Some(title) = image.title().as_option() {
                    KindData::Image(rushdown::ast::Image::inline_with_title(
                        rewrite.destination.clone(),
                        title.clone(),
                    ))
                } else {
                    KindData::Image(rushdown::ast::Image::inline(rewrite.destination.clone()))
                }),
                _ => None,
            }) else {
                continue;
            };
            *doc.arena[rewrite.node_ref].kind_data_mut() = new_kind;
            continue;
        }

        let Some(new_kind) = (match doc.arena[rewrite.node_ref].kind_data() {
            KindData::Link(link) => Some(match link.link_kind() {
                LinkKind::Auto(auto) => {
                    KindData::Link(Link::auto(rewrite.destination.clone(), auto.text().clone()))
                }
                _ => {
                    if let Some(title) = link.title().as_option() {
                        KindData::Link(Link::inline_with_title(
                            rewrite.destination.clone(),
                            title.clone(),
                        ))
                    } else {
                        KindData::Link(Link::inline(rewrite.destination.clone()))
                    }
                }
            }),
            _ => None,
        }) else {
            continue;
        };
        *doc.arena[rewrite.node_ref].kind_data_mut() = new_kind;
    }
}

fn collect_doc_nodes(arena: &rushdown::ast::Arena, node_ref: NodeRef, out: &mut Vec<NodeRef>) {
    for child in arena[node_ref].children(arena) {
        out.push(child);
        collect_doc_nodes(arena, child, out);
    }
}

fn rewrite_target_for_output(target: &str) -> String {
    let target = target.trim();
    let unwrapped = if target.starts_with('<') && target.ends_with('>') && target.len() >= 2 {
        &target[1..target.len() - 1]
    } else {
        target
    };

    if unwrapped.starts_with('#') || unwrapped.contains("://") || unwrapped.starts_with("mailto:") {
        return unwrapped.to_string();
    }

    if unwrapped.starts_with("static/") || unwrapped.starts_with('/') {
        let base_relative = unwrapped
            .strip_prefix("static/")
            .unwrap_or_else(|| unwrapped.trim_start_matches('/'));
        return base_relative.replace('\\', "/");
    }

    unwrapped.to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use anyhow::Result;
    use tempfile::TempDir;

    use crate::cli::BrokenLinkMode;
    use crate::conditional::{EffectiveFormat, RenderProfile};
    use crate::graph::{prepare_document_with_options, PrepareOptions};
    use crate::DebugDumper;

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    #[test]
    fn rewrites_link_and_image_targets_before_formatting() -> Result<()> {
        let temp = TempDir::new()?;
        let root = temp.path();
        write(
            &root.join("index.md"),
            "# Root\n\n:::include\n# [Builder](builder/index.md)\n:::\n",
        );
        write(
            &root.join("builder/index.md"),
            concat!(
                "# Builder\n\n",
                "[file-ref][f] ![img-ref][i] [absolute](/shared/manual.pdf)\n\n",
                "[f]: file.pdf\n",
                "[i]: image.png\n",
            ),
        );
        write(&root.join("builder/file.pdf"), "%PDF-1.7\n");
        write(&root.join("builder/image.png"), "png\n");
        write(&root.join("shared/manual.pdf"), "%PDF-1.7\n");

        let mut prepared = prepare_document_with_options(
            &DebugDumper::new(false),
            root,
            &root.join("index.md"),
            None,
            BrokenLinkMode::Error,
            PrepareOptions {
                transform_diagram_images: false,
            },
        )?;
        let profile =
            RenderProfile::from_metadata(EffectiveFormat::Markdown, &prepared.document_metadata())?;

        let renderer = super::MarkdownRenderer::new(profile);
        let rendered = renderer.render_document(&mut prepared)?;

        assert!(rendered.contains("[file-ref](builder/file.pdf)"));
        assert!(rendered.contains("![img-ref](builder/image.png)"));
        assert!(rendered.contains("[absolute](shared/manual.pdf)"));
        assert!(!rendered.contains("[f]:"));
        assert!(!rendered.contains("[i]:"));
        assert!(!rendered.contains("static/"));

        Ok(())
    }

    #[test]
    fn rewrites_asset_paths_as_base_relative() -> Result<()> {
        let temp = TempDir::new()?;
        let root = temp.path();
        write(
            &root.join("index.md"),
            "# Root\n\n:::include\n## [Builder](builder/index.md)\n:::\n",
        );
        write(
            &root.join("builder/index.md"),
            "# Builder\n\n[file](file.pdf)\n",
        );
        write(&root.join("builder/file.pdf"), "%PDF-1.7\n");

        let mut prepared = prepare_document_with_options(
            &DebugDumper::new(false),
            root,
            &root.join("index.md"),
            None,
            BrokenLinkMode::Error,
            PrepareOptions {
                transform_diagram_images: false,
            },
        )?;
        let profile =
            RenderProfile::from_metadata(EffectiveFormat::Markdown, &prepared.document_metadata())?;
        let renderer = super::MarkdownRenderer::new(profile);
        let rendered = renderer.render_document(&mut prepared)?;

        assert!(rendered.contains("## Builder"));
        assert!(rendered.contains("[file](builder/file.pdf)"));
        assert!(!rendered.contains("../"));

        Ok(())
    }

    #[test]
    fn normalizes_static_and_absolute_asset_targets_to_relative() {
        assert_eq!(
            super::rewrite_target_for_output("static/assets/cat.svg"),
            "assets/cat.svg"
        );
        assert_eq!(
            super::rewrite_target_for_output("/assets/cat.svg"),
            "assets/cat.svg"
        );
        assert_eq!(super::rewrite_target_for_output("#anchor"), "#anchor");
        assert_eq!(
            super::rewrite_target_for_output("https://example.com/file.png"),
            "https://example.com/file.png"
        );
    }
}
