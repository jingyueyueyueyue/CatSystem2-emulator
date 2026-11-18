pub use cs2_formats::{CstlError, CstlMessage, CstlTable};

/// A language selection resolved against the order stored in a CSTL file.
///
/// The engine uses the table's language names as opaque UTF-8 identifiers.
/// They are not converted to Windows code pages and are not treated as
/// numeric language ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CstlLanguageSelection {
    requested: Option<String>,
    selected_index: Option<usize>,
    fallback_index: Option<usize>,
}

impl CstlLanguageSelection {
    pub fn new(table: &CstlTable, requested: Option<&str>, fallback: Option<&str>) -> Self {
        let selected_index =
            requested.and_then(|name| table.languages.iter().position(|value| value == name));
        let fallback_index =
            fallback.and_then(|name| table.languages.iter().position(|value| value == name));
        Self {
            requested: requested.map(str::to_owned),
            selected_index,
            fallback_index,
        }
    }

    pub fn requested(&self) -> Option<&str> {
        self.requested.as_deref()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    pub fn fallback_index(&self) -> Option<usize> {
        self.fallback_index
    }

    pub fn active_index(&self) -> Option<usize> {
        self.selected_index.or(self.fallback_index)
    }

    pub fn set_requested(&mut self, table: &CstlTable, requested: Option<&str>) {
        self.requested = requested.map(str::to_owned);
        self.selected_index =
            requested.and_then(|name| table.languages.iter().position(|value| value == name));
    }

    pub fn set_fallback(&mut self, table: &CstlTable, fallback: Option<&str>) {
        self.fallback_index =
            fallback.and_then(|name| table.languages.iter().position(|value| value == name));
    }
}

/// CSTL lookup state used by a CST interpreter.
///
/// `fallback` is explicit. If the selected language is absent from the table,
/// the lookup may use it. With a bound language, names and message fragments
/// preserve empty fields and return empty text for missing rows/fragments.
/// Choice text separately falls back to CST when the collected text is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CstlLocalization {
    table: CstlTable,
    selection: CstlLanguageSelection,
}

impl CstlLocalization {
    pub fn new(table: CstlTable, requested: Option<&str>, fallback: Option<&str>) -> Self {
        let selection = CstlLanguageSelection::new(&table, requested, fallback);
        Self { table, selection }
    }

    pub fn table(&self) -> &CstlTable {
        &self.table
    }

    pub fn selection(&self) -> &CstlLanguageSelection {
        &self.selection
    }

    pub fn set_language(&mut self, language: Option<&str>) {
        self.selection.set_requested(&self.table, language);
    }

    pub fn set_fallback(&mut self, language: Option<&str>) {
        self.selection.set_fallback(&self.table, language);
    }

    pub fn language(&self) -> Option<&str> {
        self.selection
            .active_index()
            .and_then(|index| self.table.languages.get(index))
            .map(String::as_str)
    }

    pub fn message(&self, input: usize) -> Option<&CstlMessage> {
        self.selection
            .active_index()
            .and_then(|language| self.table.messages.get(input)?.get(language))
    }

    pub fn name(&self, input: usize) -> Option<&str> {
        self.language()?;
        Some(self.message(input).map_or("", |message| message.name.as_str()))
    }

    pub fn content_fragment(&self, input: usize, fragment: usize) -> Option<&str> {
        self.language()?;
        Some(
            self.message(input)
                .and_then(|message| message.content.split('\u{1f}').nth(fragment))
                .unwrap_or(""),
        )
    }

    pub fn name_bytes(&self, input: usize, fallback: &[u8]) -> Vec<u8> {
        self.name(input)
            .map_or_else(|| fallback.to_vec(), |name| name.as_bytes().to_vec())
    }

    pub fn content_bytes(&self, input: usize, fragment: i32, fallback: &[u8]) -> Vec<u8> {
        if self.language().is_none() {
            return fallback.to_vec();
        }
        self.message(input)
            .and_then(|message| split_fragment(&message.content, fragment))
            .unwrap_or("")
            .as_bytes()
            .to_vec()
    }

    /// CSTL choice rows start after the final CST input interval. Conditions
    /// are evaluated by the CST interpreter, so `choice` is already dense.
    pub fn option_bytes(&self, input_count: usize, choice: usize, fallback: &[u8]) -> Vec<u8> {
        let index = input_count
            .checked_sub(1)
            .and_then(|base| base.checked_add(choice));
        let Some(index) = index else {
            return fallback.to_vec();
        };
        let Some(message) = self.message(index) else {
            return fallback.to_vec();
        };
        let value = message
            .content
            .split('\u{1f}')
            .take_while(|part| !part.is_empty())
            .collect::<String>();
        if value.is_empty() {
            fallback.to_vec()
        } else {
            value.into_bytes()
        }
    }

    pub fn option(&self, input_count: usize, choice: usize) -> Option<String> {
        let index = input_count.checked_sub(1)?.checked_add(choice)?;
        let message = self.message(index)?;
        let value = message
            .content
            .split('\u{1f}')
            .take_while(|part| !part.is_empty())
            .collect::<String>();
        (!value.is_empty()).then_some(value)
    }
}

fn split_fragment(content: &str, fragment: i32) -> Option<&str> {
    if fragment < 0 {
        return Some(content);
    }
    content.split('\u{1f}').nth(fragment as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> CstlTable {
        CstlTable {
            languages: vec!["ja".into(), "zh".into()],
            messages: vec![
                vec![
                    CstlMessage {
                        name: "名前".into(),
                        content: "first\u{1f}second".into(),
                    },
                    CstlMessage {
                        name: "姓名".into(),
                        content: "第一\u{1f}第二".into(),
                    },
                ],
                vec![
                    CstlMessage {
                        name: String::new(),
                        content: String::new(),
                    },
                    CstlMessage {
                        name: String::new(),
                        content: "选择一".into(),
                    },
                ],
            ],
        }
    }

    #[test]
    fn selection_uses_named_language_and_explicit_fallback() {
        let localization = CstlLocalization::new(table(), Some("missing"), Some("zh"));
        assert_eq!(localization.language(), Some("zh"));
        assert_eq!(localization.name_bytes(0, b"fallback"), "姓名".as_bytes());
        assert_eq!(
            localization.content_bytes(0, 1, b"fallback"),
            "第二".as_bytes()
        );
    }

    #[test]
    fn fragments_preserve_empty_text_and_choices_keep_cst_fallbacks() {
        let localization = CstlLocalization::new(table(), Some("ja"), None);
        assert_eq!(
            localization.content_bytes(0, -1, b"fallback"),
            b"first\x1fsecond"
        );
        assert_eq!(
            localization.option_bytes(2, 0, b"fallback"),
            "选择一".as_bytes()
        );
        assert_eq!(localization.option_bytes(2, 2, b"fallback"), b"fallback");
        assert_eq!(localization.name_bytes(1, b"fallback"), b"");
        assert_eq!(localization.content_bytes(0, 2, b"fallback"), b"");
        assert_eq!(localization.name_bytes(99, b"fallback"), b"");
        assert_eq!(localization.content_bytes(99, 0, b"fallback"), b"");
        assert_eq!(localization.name(1), Some(""));
        assert_eq!(localization.content_fragment(0, 2), Some(""));
    }

    #[test]
    fn an_unbound_language_retains_original_cst_text() {
        let localization = CstlLocalization::new(table(), Some("missing"), None);
        assert_eq!(localization.name(0), None);
        assert_eq!(localization.content_fragment(0, 0), None);
        assert_eq!(localization.name_bytes(0, b"original"), b"original");
        assert_eq!(localization.content_bytes(0, 0, b"original"), b"original");
    }

    #[test]
    fn interpreter_emits_empty_bound_fields_instead_of_original_text() {
        let mut interpreter = crate::CstInterpreter::new(crate::CstDocument {
            records: vec![
                crate::CstRecord { kind: 0x21, text: b"original name".to_vec() },
                crate::CstRecord { kind: 0x20, text: b"original first".to_vec() },
                crate::CstRecord { kind: 0x20, text: b"original second".to_vec() },
            ],
            inputs: vec![0..3],
        });
        interpreter.set_localization(Some(CstlLocalization::new(
            CstlTable {
                languages: vec!["en".into()],
                messages: vec![vec![CstlMessage {
                    name: String::new(),
                    content: "translated first".into(),
                }]],
            },
            Some("en"),
            None,
        )));
        let mut state = crate::expression::ExpressionState::default();
        interpreter.run(&mut state, 1).unwrap();
        assert_eq!(interpreter.pending(), Some(&crate::CstEvent::LocalizedSpeaker {
            text: Vec::new(), language: "en".into(),
        }));
        interpreter.resume();
        interpreter.run(&mut state, 1).unwrap();
        assert_eq!(interpreter.pending(), Some(&crate::CstEvent::LocalizedMessage {
            text: b"translated first".to_vec(), language: "en".into(),
        }));
        interpreter.resume();
        interpreter.run(&mut state, 1).unwrap();
        assert_eq!(interpreter.pending(), Some(&crate::CstEvent::LocalizedMessage {
            text: Vec::new(), language: "en".into(),
        }));
    }
}
