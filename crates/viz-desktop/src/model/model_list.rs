//! The model picker's list: rows for the open rail tab, cross-harness search
//! results grouped by harness, or starred favourites, with sub-provider
//! headings and "Show more" folds for long groups.

use std::borrow::Cow;

use viz_client::protocol::{ConfigOption, OptionChoice};

use super::{
    Catalog, Model, ModelMenuState, ModelRef, PickerScope, RailTab, catalogs, choice_group,
    model_name, picker_scope, provider_name, status_usable,
};

/// Groups longer than this fold behind "Show more" while browsing.
const FOLD_LIMIT: usize = 8;
/// Models a folded group still shows.
const FOLD_SHOWN: usize = 6;

/// A model row in the list.
#[derive(Debug, Clone)]
pub struct ModelRow<'a> {
    /// Which model.
    pub model: ModelRef,
    /// Its harness's display name.
    pub harness_name: &'a str,
    /// The model choice.
    pub choice: &'a OptionChoice,
    /// Display name.
    pub label: &'a str,
    /// Sub-provider tag shown in mixed lists.
    pub tag: Option<Cow<'a, str>>,
    /// The scope's current model.
    pub selected: bool,
    /// Starred.
    pub favorite: bool,
    /// Cannot be chosen (another harness on a thread, or harness unusable).
    pub disabled: bool,
}

/// An entry in the model list.
#[derive(Debug, Clone)]
pub enum ListItem<'a> {
    /// Sub-provider or harness heading.
    Header {
        /// Element key.
        key: String,
        /// Heading text.
        label: Cow<'a, str>,
    },
    /// A model.
    Model(ModelRow<'a>),
    /// "Show more" / "Show less" toggle for a long group.
    Fold {
        /// `harness/group` key toggled in [`ModelMenuState::expanded`].
        key: String,
        /// Whether the group is expanded.
        expanded: bool,
        /// Models hidden while folded.
        hidden: usize,
    },
}

/// Why the list is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListEmpty {
    /// Favorites tab with nothing starred.
    NoFavorites,
    /// Search with no matches.
    NoMatches,
    /// A harness that has not reported models yet.
    Loading,
    /// A harness without a model selector.
    NoModels,
}

/// The list the model column shows.
#[derive(Debug, Clone)]
pub struct PickerList<'a> {
    /// The scope.
    pub scope: PickerScope<'a>,
    /// Every catalog, rail order.
    pub catalogs: Vec<Catalog<'a>>,
    /// The tab shown, resolved.
    pub tab: RailTab,
    /// Whether a search spans every harness.
    pub searching: bool,
    /// Rows, headings and folds.
    pub items: Vec<ListItem<'a>>,
    /// Why there are no rows.
    pub empty: Option<ListEmpty>,
}

impl<'a> PickerList<'a> {
    /// Model rows in display order.
    pub fn rows(&self) -> impl Iterator<Item = &ModelRow<'a>> {
        self.items.iter().filter_map(|item| match item {
            ListItem::Model(row) => Some(row),
            ListItem::Header { .. } | ListItem::Fold { .. } => None,
        })
    }

    /// The previewed row: highlighted, else selected, else first.
    #[must_use]
    pub fn preview(&self, state: &ModelMenuState) -> Option<&ModelRow<'a>> {
        let find = |wanted: &ModelRef| self.rows().find(|row| row.model == *wanted);
        state
            .highlight
            .as_ref()
            .and_then(find)
            .or_else(|| self.rows().find(|row| row.selected))
            .or_else(|| self.rows().next())
    }

    /// The catalog of a harness id.
    #[must_use]
    pub fn catalog(&self, id: &str) -> Option<&Catalog<'a>> {
        self.catalogs.iter().find(|catalog| catalog.id == id)
    }
}

/// Derives the model list for the open picker.
#[must_use]
pub fn picker_list<'a>(model: &'a Model) -> Option<PickerList<'a>> {
    let scope = picker_scope(model)?;
    let state = &model.composer.model_menu;
    let catalogs = catalogs(model, &scope);
    let selected = scope.selected();
    let favorites = &model.composer.favorite_models;
    let query = state.query.trim();
    let searching = !query.is_empty();
    let tab = state
        .tab
        .clone()
        .filter(|tab| match tab {
            RailTab::Favorites => true,
            RailTab::Harness(id) => {
                (!scope.locked || id == scope.harness)
                    && catalogs.iter().any(|catalog| catalog.id == id)
            }
        })
        .unwrap_or_else(|| RailTab::Harness(scope.harness.to_owned()));
    let row = |catalog: &Catalog<'a>, choice: &'a OptionChoice| {
        let model = ModelRef {
            harness: catalog.id.to_owned(),
            value: choice.value.clone(),
        };
        ModelRow {
            selected: selected.as_ref() == Some(&model),
            favorite: favorites.contains(&model.favorite_key()),
            disabled: (scope.locked && !catalog.own) || !status_usable(catalog.status),
            harness_name: catalog.name,
            label: model_name(choice),
            tag: None,
            model,
            choice,
        }
    };
    let mut items = Vec::new();
    let mut empty = None;
    if searching {
        searched(&catalogs, scope.locked, query, &row, &mut items);
        empty = items.is_empty().then_some(ListEmpty::NoMatches);
    } else if tab == RailTab::Favorites {
        for key in favorites {
            let Some(wanted) = ModelRef::from_favorite(key) else {
                continue;
            };
            if scope.locked && wanted.harness != scope.harness {
                continue;
            }
            let found = catalogs
                .iter()
                .find(|c| c.id == wanted.harness)
                .and_then(|catalog| {
                    let option = catalog.model_option()?;
                    let choice = option.choices.iter().find(|c| c.value == wanted.value)?;
                    Some(row(catalog, choice))
                });
            items.extend(found.map(ListItem::Model));
        }
        empty = items.is_empty().then_some(ListEmpty::NoFavorites);
    } else if let RailTab::Harness(id) = &tab
        && let Some(catalog) = catalogs.iter().find(|catalog| catalog.id == id)
    {
        match catalog.model_option() {
            Some(option) => grouped(catalog, option, state, &row, &mut items),
            None if catalog.options.is_empty() && status_usable(catalog.status) => {
                empty = Some(ListEmpty::Loading);
            }
            None => empty = Some(ListEmpty::NoModels),
        }
    }
    Some(PickerList {
        scope,
        catalogs,
        tab,
        searching,
        items,
        empty,
    })
}

fn grouped<'a>(
    catalog: &Catalog<'a>,
    option: &'a ConfigOption,
    state: &ModelMenuState,
    row: &impl Fn(&Catalog<'a>, &'a OptionChoice) -> ModelRow<'a>,
    items: &mut Vec<ListItem<'a>>,
) {
    let mut groups: Vec<(Option<&str>, Vec<&OptionChoice>)> = Vec::new();
    for choice in &option.choices {
        let group = choice_group(choice);
        match groups.iter_mut().find(|(name, _)| *name == group) {
            Some((_, choices)) => choices.push(choice),
            None => groups.push((group, vec![choice])),
        }
    }
    let headed = groups.len() > 1;
    for (group, choices) in groups {
        let key = format!("{}/{}", catalog.id, group.unwrap_or_default());
        if headed && let Some(group) = group {
            items.push(ListItem::Header {
                key: format!("model-heading-{key}"),
                label: provider_name(group),
            });
        }
        let folds = choices.len() > FOLD_LIMIT;
        let expanded = state.expanded.contains(&key);
        let mut hidden = 0;
        for (index, choice) in choices.iter().enumerate() {
            let row = row(catalog, choice);
            if folds && !expanded && index >= FOLD_SHOWN && !row.selected {
                hidden += 1;
            } else {
                items.push(ListItem::Model(row));
            }
        }
        if folds {
            items.push(ListItem::Fold {
                key,
                expanded,
                hidden,
            });
        }
    }
}

fn searched<'a>(
    catalogs: &[Catalog<'a>],
    locked: bool,
    query: &str,
    row: &impl Fn(&Catalog<'a>, &'a OptionChoice) -> ModelRow<'a>,
    items: &mut Vec<ListItem<'a>>,
) {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    for catalog in catalogs {
        if locked && !catalog.own {
            continue;
        }
        let Some(option) = catalog.model_option() else {
            continue;
        };
        let matches: Vec<_> = option
            .choices
            .iter()
            .filter(|choice| {
                let group = choice_group(choice).unwrap_or_default();
                let haystack = format!(
                    "{} {} {group} {} {}",
                    catalog.name,
                    provider_name(group),
                    model_name(choice),
                    choice.value
                )
                .to_lowercase();
                words.iter().all(|word| haystack.contains(word.as_str()))
            })
            .collect();
        if matches.is_empty() {
            continue;
        }
        items.push(ListItem::Header {
            key: format!("model-heading-{}", catalog.id),
            label: Cow::Borrowed(catalog.name),
        });
        items.extend(matches.into_iter().map(|choice| {
            let mut row = row(catalog, choice);
            row.tag = choice_group(choice).map(provider_name);
            ListItem::Model(row)
        }));
    }
}
