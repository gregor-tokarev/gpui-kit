use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use anyhow::{Context as _, Result};
use gpui::SharedString;
use tree_sitter::{Language, Query};

use super::GrammarConfig;

#[derive(Clone, PartialEq, Eq, Hash)]
struct QueryKey {
    grammar: Language,
    injections: SharedString,
    locals: SharedString,
    highlights: SharedString,
}

pub(super) struct LanguageQueries {
    pub query: Arc<Query>,
    pub injections: Option<Arc<Query>>,
    pub locals_pattern_index: usize,
    pub highlights_pattern_index: usize,
}

/// Queries are immutable after configuration. Keep them across editor instances;
/// parsers, source text and syntax trees remain owned by each highlighter.
pub(super) fn language_queries(
    grammar: &Language,
    config: &GrammarConfig,
) -> Result<Arc<LanguageQueries>> {
    static CACHE: LazyLock<Mutex<HashMap<QueryKey, Arc<LanguageQueries>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    // Include the actual grammar and query sources, rather than the language
    // name: registering a replacement grammar or queries must not reuse stale
    // captures. This also lets aliases share the same compiled queries.
    let key = QueryKey {
        grammar: grammar.clone(),
        injections: config.injections.clone(),
        locals: config.locals.clone(),
        highlights: config.highlights.clone(),
    };

    if let Some(queries) = CACHE.lock().unwrap().get(&key).cloned() {
        return Ok(queries);
    }

    // Compile outside the lock so background warm-up never holds up unrelated
    // editors. A concurrent cache miss can compile its own copy safely.
    let mut source = config.injections.to_string();
    let locals_offset = source.len();
    source.push_str(&config.locals);
    let highlights_offset = source.len();
    source.push_str(&config.highlights);
    let mut query = Query::new(grammar, &source).context("new query")?;
    let mut locals_pattern_index = 0;
    let mut highlights_pattern_index = 0;

    for index in 0..query.pattern_count() {
        let offset = query.start_byte_for_pattern(index);
        if offset < highlights_offset {
            highlights_pattern_index += 1;
        }
        if offset < locals_offset {
            locals_pattern_index += 1;
        }
    }

    // Injections are parsed separately and must not produce host highlights.
    for index in 0..locals_pattern_index {
        query.disable_pattern(index);
    }

    let injections = if config.injections.is_empty() {
        None
    } else {
        Query::new(grammar, &config.injections).ok().map(Arc::new)
    };
    let queries = Arc::new(LanguageQueries {
        query: Arc::new(query),
        injections,
        locals_pattern_index,
        highlights_pattern_index,
    });

    Ok(CACHE.lock().unwrap().entry(key).or_insert(queries).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlighter::LanguageRegistry;

    fn config() -> GrammarConfig {
        let mut config = LanguageRegistry::singleton().language("json").unwrap();
        config.highlights = "(string) @string".into();
        config.injections = SharedString::default();
        config.locals = SharedString::default();
        config
    }

    #[test]
    fn editors_and_language_aliases_share_queries() {
        let mut config = config();
        let grammar = config.language.clone().unwrap();
        let first = language_queries(&grammar, &config).unwrap();
        config.name = "json-alias".into();
        let second = language_queries(&grammar, &config).unwrap();

        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn background_warmup_is_reused_by_another_thread() {
        let mut config = config();
        config.highlights = "(number) @number".into();
        let grammar = config.language.clone().unwrap();
        let background_config = config.clone();
        let background_grammar = grammar.clone();
        let warmed = std::thread::spawn(move || {
            language_queries(&background_grammar, &background_config).unwrap()
        })
        .join()
        .unwrap();
        let editor = language_queries(&grammar, &config).unwrap();

        assert!(Arc::ptr_eq(&warmed, &editor));
    }

    #[test]
    fn replaced_queries_do_not_reuse_old_captures() {
        let mut config = config();
        let grammar = config.language.clone().unwrap();
        let first = language_queries(&grammar, &config).unwrap();
        config.highlights = "(string) @constant".into();
        let replaced = language_queries(&grammar, &config).unwrap();

        assert!(!Arc::ptr_eq(&first, &replaced));
        assert_eq!(first.query.capture_names(), &["string"]);
        assert_eq!(replaced.query.capture_names(), &["constant"]);
    }

    #[cfg(feature = "tree-sitter-javascript")]
    #[test]
    fn replaced_grammars_do_not_reuse_queries() {
        let config = config();
        let first = language_queries(config.language.as_ref().unwrap(), &config).unwrap();
        let grammar = LanguageRegistry::singleton()
            .language("javascript")
            .unwrap()
            .language
            .unwrap();
        let other_grammar = language_queries(&grammar, &config).unwrap();

        assert!(!Arc::ptr_eq(&first, &other_grammar));
    }

    #[test]
    fn invalid_queries_do_not_poison_the_cache() {
        let mut config = config();
        let grammar = config.language.clone().unwrap();
        config.highlights = "(not_a_json_node) @string".into();
        assert!(language_queries(&grammar, &config).is_err());

        config.highlights = "(string) @string".into();
        assert!(language_queries(&grammar, &config).is_ok());
    }
}
