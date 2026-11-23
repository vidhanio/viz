//! Convert an edited project setting into a typed server patch.

use viz_client::protocol::{ProjectAutoSettle, ProjectIconChange, ProjectSettingsPatch, Setting};

use crate::{effects::projects::ProjectRequest, model::projects::Field};

pub(super) fn edit(project: String, field: &Field, value: &str) -> Result<ProjectRequest, String> {
    let mut patch = ProjectSettingsPatch::default();
    let mut name = None;
    let setting = || {
        Some(if value.is_empty() {
            Setting::Clear
        } else {
            Setting::Set(value.into())
        })
    };
    match field {
        Field::Name if value.is_empty() => return Err("Enter a project name.".into()),
        Field::Name => name = Some(value.into()),
        Field::Harness => patch.default_harness = setting(),
        Field::Model => patch.default_model = setting(),
        Field::Options => {
            patch.default_options = Some(if value.is_empty() {
                std::collections::BTreeMap::new()
            } else {
                serde_json::from_str(value)
                    .map_err(|_| "Enter a JSON object whose values are strings or booleans.")?
            });
        }
        Field::Base => patch.base_branch = setting(),
        Field::Prefix => patch.branch_prefix = setting(),
        Field::Settle => {
            let days = value
                .parse::<u16>()
                .ok()
                .filter(|days| *days > 0)
                .ok_or("Enter a positive number of days.")?;
            patch.auto_settle = Some(Setting::Set(ProjectAutoSettle::AfterDays { days }));
        }
        Field::Emoji if value.is_empty() => return Err("Enter an emoji.".into()),
        Field::Emoji => {
            patch.icon = Some(ProjectIconChange::Emoji {
                emoji: value.into(),
            });
        }
    }
    Ok(ProjectRequest::Save {
        project,
        name,
        patch: Box::new(patch),
    })
}
