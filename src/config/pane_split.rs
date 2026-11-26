//! Pane split templates: the built-in layouts, and parsing the
//! `paneSplitTemplates` section of `config.json` into them — the panes, their
//! stacked commands, environments and overlays.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneSplitTemplate {
    Pane(Box<PaneSplitPane>),
    Split {
        axis: PaneSplitAxis,
        first: Box<PaneSplitTemplate>,
        second: Box<PaneSplitTemplate>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSplitTemplateConfig {
    pub layout: PaneSplitTemplate,
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneSplitPane {
    pub label: Option<String>,
    pub profile: Option<Profile>,
    pub command: Option<PaneSplitCommand>,
    pub theme: Option<String>,
    pub dark_theme: Option<String>,
    pub env: HashMap<String, String>,
    pub overlay: Option<PaneSplitOverlay>,
    /// Commands seeded as stacked entries in this pane, sharing its layout
    /// region with the interactive shell.
    pub stack: Vec<PaneSplitCommand>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSplitCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl PaneSplitCommand {
    pub fn shell(&self) -> Shell {
        Shell::WithArguments {
            program: self.program.clone(),
            args: self.args.clone(),
            title_override: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSplitOverlay {
    pub text: Option<String>,
    pub size: Option<OverlayFontSize>,
    pub opacity: Option<u8>,
    pub color: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneSplitAxis {
    Horizontal,
    Vertical,
}

impl From<PaneSplitAxis> for zetta_layout::SplitAxis {
    fn from(axis: PaneSplitAxis) -> Self {
        match axis {
            PaneSplitAxis::Horizontal => Self::Horizontal,
            PaneSplitAxis::Vertical => Self::Vertical,
        }
    }
}

impl PaneSplitAxis {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Horizontal => "Horizontal",
            Self::Vertical => "Vertical",
        }
    }
}

impl PaneSplitTemplate {
    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane(_) => 1,
            Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    /// The stacked commands the whole template seeds. Each one becomes its own
    /// terminal, so it counts against the same budget as a layout pane.
    pub fn stacked_command_count(&self) -> usize {
        match self {
            Self::Pane(pane) => pane.stack.len(),
            Self::Split { first, second, .. } => {
                first.stacked_command_count() + second.stacked_command_count()
            }
        }
    }

    #[cfg(test)]
    pub fn pane_labels(&self) -> Vec<Option<String>> {
        let mut labels = Vec::with_capacity(self.pane_count());
        self.collect_pane_labels(&mut labels);
        labels
    }

    #[cfg(test)]
    pub(super) fn collect_pane_labels(&self, labels: &mut Vec<Option<String>>) {
        match self {
            Self::Pane(pane) => labels.push(pane.label.clone()),
            Self::Split { first, second, .. } => {
                first.collect_pane_labels(labels);
                second.collect_pane_labels(labels);
            }
        }
    }

    pub fn pane_specifications(&self) -> Vec<PaneSplitPane> {
        let mut panes = Vec::with_capacity(self.pane_count());
        self.collect_pane_specifications(&mut panes);
        panes
    }

    pub(super) fn collect_pane_specifications(&self, panes: &mut Vec<PaneSplitPane>) {
        match self {
            Self::Pane(pane) => panes.push((**pane).clone()),
            Self::Split { first, second, .. } => {
                first.collect_pane_specifications(panes);
                second.collect_pane_specifications(panes);
            }
        }
    }
}

impl PaneSplitTemplateConfig {
    pub fn pane_count(&self) -> usize {
        self.layout.pane_count()
    }

    #[cfg(test)]
    pub fn pane_labels(&self) -> Vec<Option<String>> {
        self.layout.pane_labels()
    }

    pub fn pane_specifications(&self) -> Vec<PaneSplitPane> {
        self.layout
            .pane_specifications()
            .into_iter()
            .map(|mut pane| {
                let pane_environment = std::mem::take(&mut pane.env);
                pane.env = self.env.clone();
                pane.env.extend(pane_environment);
                pane
            })
            .collect()
    }
}

pub(crate) fn built_in_pane_split_templates() -> HashMap<String, PaneSplitTemplateConfig> {
    let labeled_pane = |label: &str| {
        Box::new(PaneSplitTemplate::Pane(Box::new(PaneSplitPane {
            label: Some(label.to_owned()),
            ..PaneSplitPane::default()
        })))
    };
    HashMap::from([
        (
            "three-right".to_owned(),
            PaneSplitTemplateConfig {
                env: HashMap::new(),
                layout: PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Vertical,
                    first: labeled_pane("left"),
                    second: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Horizontal,
                        first: labeled_pane("top-right"),
                        second: labeled_pane("bottom-right"),
                    }),
                },
            },
        ),
        (
            "three-left".to_owned(),
            PaneSplitTemplateConfig {
                env: HashMap::new(),
                layout: PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Vertical,
                    first: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Horizontal,
                        first: labeled_pane("top-left"),
                        second: labeled_pane("bottom-left"),
                    }),
                    second: labeled_pane("right"),
                },
            },
        ),
        (
            "quarters".to_owned(),
            PaneSplitTemplateConfig {
                env: HashMap::new(),
                layout: PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Vertical,
                    first: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Horizontal,
                        first: labeled_pane("top-left"),
                        second: labeled_pane("bottom-left"),
                    }),
                    second: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Horizontal,
                        first: labeled_pane("top-right"),
                        second: labeled_pane("bottom-right"),
                    }),
                },
            },
        ),
        (
            "four-vertical".to_owned(),
            PaneSplitTemplateConfig {
                env: HashMap::new(),
                layout: PaneSplitTemplate::Split {
                    axis: PaneSplitAxis::Vertical,
                    first: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Vertical,
                        first: labeled_pane("left"),
                        second: labeled_pane("left-center"),
                    }),
                    second: Box::new(PaneSplitTemplate::Split {
                        axis: PaneSplitAxis::Vertical,
                        first: labeled_pane("right-center"),
                        second: labeled_pane("right"),
                    }),
                },
            },
        ),
    ])
}

pub(super) fn default_pane_split_templates() -> HashMap<String, PaneSplitTemplateConfig> {
    built_in_pane_split_templates()
}

pub(super) fn parse_pane_split_template_config(
    value: &Value,
    profiles: &[Profile],
) -> Result<PaneSplitTemplateConfig> {
    let object = value
        .as_object()
        .context("pane split templates must be objects")?;
    const FIELDS: &[&str] = &["layout", "env"];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized pane split template field {field:?}");
    }
    let layout = object
        .get("layout")
        .context("pane split template layout is required")?;
    let env = object
        .get("env")
        .map(parse_pane_split_environment)
        .transpose()?
        .unwrap_or_default();
    let layout = parse_pane_split_template(layout, profiles)?;
    // Panes and stacked commands are both terminals, so stacked commands cannot
    // push a template past what a tab can hold. The pane count on its own is
    // validated by the caller, which owns the name for that error.
    let stacked_commands = layout.stacked_command_count();
    anyhow::ensure!(
        stacked_commands == 0 || layout.pane_count() + stacked_commands <= MAX_PANES_PER_TAB,
        "pane split templates must not declare more than {MAX_PANES_PER_TAB} panes and stacked commands combined"
    );
    Ok(PaneSplitTemplateConfig { layout, env })
}

pub(super) fn parse_pane_split_template(
    value: &Value,
    profiles: &[Profile],
) -> Result<PaneSplitTemplate> {
    let object = value
        .as_object()
        .context("template nodes must be a pane object or a split object")?;

    if let Some(axis_name) = object
        .keys()
        .find(|key| matches!(key.as_str(), "horizontal" | "vertical"))
    {
        anyhow::ensure!(
            object.len() == 1,
            "split objects must contain exactly one axis"
        );
        let axis = match axis_name.as_str() {
            "horizontal" => PaneSplitAxis::Horizontal,
            "vertical" => PaneSplitAxis::Vertical,
            _ => unreachable!(),
        };
        let children = object
            .get(axis_name)
            .expect("the selected split axis must be present")
            .as_array()
            .context("split children must be a two-element array")?;
        anyhow::ensure!(children.len() == 2, "splits must have exactly two children");
        return Ok(PaneSplitTemplate::Split {
            axis,
            first: Box::new(parse_pane_split_template(&children[0], profiles)?),
            second: Box::new(parse_pane_split_template(&children[1], profiles)?),
        });
    }

    parse_pane_split_pane(object, profiles).map(|pane| PaneSplitTemplate::Pane(Box::new(pane)))
}

pub(super) fn parse_pane_split_pane(
    object: &serde_json::Map<String, Value>,
    profiles: &[Profile],
) -> Result<PaneSplitPane> {
    const FIELDS: &[&str] = &[
        "label",
        "profile",
        "command",
        "theme",
        "dark_theme",
        "env",
        "overlay",
        "stack",
    ];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized pane template field {field:?}");
    }

    let label = object
        .get("label")
        .map(|label| {
            let label = label
                .as_str()
                .context("pane template label must be a string")?;
            anyhow::ensure!(
                is_valid_pane_split_label(label),
                "pane labels must be lowercase kebab-case"
            );
            Ok(label.to_owned())
        })
        .transpose()?;

    let profile = object
        .get("profile")
        .map(|profile| {
            let name = profile
                .as_str()
                .context("pane template profile must be a string")?;
            profiles
                .iter()
                .find(|profile| profile.name.eq_ignore_ascii_case(name))
                .cloned()
                .with_context(|| format!("pane template profile {name:?} is not available"))
        })
        .transpose()?;

    let command = object
        .get("command")
        .map(parse_pane_split_command)
        .transpose()?;
    anyhow::ensure!(
        profile.is_none() || command.is_none(),
        "pane template profile and command are mutually exclusive"
    );

    let theme = object
        .get("theme")
        .map(|theme| {
            let theme = theme
                .as_str()
                .context("pane template theme must be a string")?;
            anyhow::ensure!(!theme.is_empty(), "pane template theme must not be empty");
            Ok(theme.to_owned())
        })
        .transpose()?;

    let dark_theme = object
        .get("dark_theme")
        .map(|dark_theme| {
            let dark_theme = dark_theme
                .as_str()
                .context("pane template dark_theme must be a string")?;
            anyhow::ensure!(
                !dark_theme.is_empty(),
                "pane template dark_theme must not be empty"
            );
            Ok(dark_theme.to_owned())
        })
        .transpose()?;

    let env = object
        .get("env")
        .map(parse_pane_split_environment)
        .transpose()?
        .unwrap_or_default();

    let overlay = object
        .get("overlay")
        .map(parse_pane_split_overlay)
        .transpose()?;

    let stack = object
        .get("stack")
        .map(parse_pane_split_stack)
        .transpose()?
        .unwrap_or_default();

    Ok(PaneSplitPane {
        label,
        profile,
        command,
        theme,
        dark_theme,
        env,
        overlay,
        stack,
    })
}

pub(super) fn parse_pane_split_stack(value: &Value) -> Result<Vec<PaneSplitCommand>> {
    let entries = value
        .as_array()
        .context("pane template stack must be an array of command objects")?;
    // A pane hosts its interactive shell plus its stacked entries, which is the
    // same limit `PaneStack::push` enforces at runtime.
    anyhow::ensure!(
        entries.len() < MAX_PANES_PER_TAB,
        "pane template stack must not contain more than {} commands",
        MAX_PANES_PER_TAB - 1
    );
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            parse_pane_split_command(entry)
                .with_context(|| format!("parsing pane template stack entry {}", index + 1))
        })
        .collect()
}

pub(super) fn parse_pane_split_command(value: &Value) -> Result<PaneSplitCommand> {
    let object = value
        .as_object()
        .context("pane template command must be an object")?;
    const FIELDS: &[&str] = &["program", "args"];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized pane template command field {field:?}");
    }
    let program = object
        .get("program")
        .and_then(Value::as_str)
        .context("pane template command.program must be a string")?
        .to_owned();
    anyhow::ensure!(
        !program.is_empty(),
        "pane template command.program must not be empty"
    );
    let args = object
        .get("args")
        .map(|args| {
            args.as_array()
                .context("pane template command.args must be an array")?
                .iter()
                .map(|arg| {
                    arg.as_str()
                        .map(str::to_owned)
                        .context("pane template command arguments must be strings")
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(PaneSplitCommand { program, args })
}

pub(super) fn parse_pane_split_environment(value: &Value) -> Result<HashMap<String, String>> {
    let object = value
        .as_object()
        .context("pane template env must be an object of strings")?;
    object
        .iter()
        .map(|(name, value)| {
            anyhow::ensure!(
                !name.is_empty() && !name.contains(['=', '\0']),
                "pane template environment variable names must not be empty or contain '='"
            );
            let value = value
                .as_str()
                .context("pane template environment values must be strings")?;
            anyhow::ensure!(
                !value.contains('\0'),
                "pane template environment values must not contain NUL"
            );
            Ok((name.clone(), value.to_owned()))
        })
        .collect()
}

pub(super) fn parse_pane_split_overlay(value: &Value) -> Result<PaneSplitOverlay> {
    let object = value
        .as_object()
        .context("pane template overlay must be an object")?;
    const FIELDS: &[&str] = &["text", "size", "opacity", "color"];
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        anyhow::bail!("unrecognized pane template overlay field {field:?}");
    }
    let text = object
        .get("text")
        .map(|text| {
            text.as_str()
                .map(str::to_owned)
                .context("pane template overlay.text must be a string")
        })
        .transpose()?;
    let size = object
        .get("size")
        .map(|size| {
            let size = size
                .as_str()
                .context("pane template overlay.size must be a string")?;
            size.parse::<OverlayFontSize>()
                .context("pane template overlay.size must be one of sm, base, lg, xl, 2xl, or 3xl")
        })
        .transpose()?;
    let opacity = object
        .get("opacity")
        .map(|opacity| {
            let opacity = opacity
                .as_u64()
                .context("pane template overlay.opacity must be an integer from 0 to 100")?;
            u8::try_from(opacity)
                .ok()
                .filter(|opacity| *opacity <= 100)
                .context("pane template overlay.opacity must be an integer from 0 to 100")
        })
        .transpose()?;
    let color = object
        .get("color")
        .map(|color| {
            let color = color
                .as_str()
                .context("pane template overlay.color must be a string")?;
            anyhow::ensure!(
                !color.trim().is_empty(),
                "pane template overlay.color must not be empty"
            );
            anyhow::ensure!(
                crate::pane::overlay_color_from_value(color).is_some(),
                "pane template overlay.color must be a named color or a valid hex color"
            );
            Ok(color.to_owned())
        })
        .transpose()?;
    Ok(PaneSplitOverlay {
        text,
        size,
        opacity,
        color,
    })
}

pub(crate) fn is_valid_pane_split_label(label: &str) -> bool {
    let mut segment_has_character = false;
    for byte in label.bytes() {
        if byte == b'-' {
            if !segment_has_character {
                return false;
            }
            segment_has_character = false;
        } else if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            segment_has_character = true;
        } else {
            return false;
        }
    }
    segment_has_character
}
