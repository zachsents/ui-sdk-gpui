//! Port of `components/slider/slider-generic.ts`: the slider's types as
//! functions of its value type, a number or a list of them.
//!
//! The original narrows the value type `T` for renderers, so `value`,
//! `defaultValue` and the callbacks agree on it. Here [`Slider`] takes it as
//! a type parameter: `f64` for one thumb, `Vec<f64>` for one number per
//! thumb. A value keeps its shape: a number stays a number and a list stays
//! a list.
//!
//! [`Slider`]: crate::Slider

use std::fmt::Debug;

use ui_sdk_kernel::{AgentArgs, AgentError, AgentValue};

/// What a slider's value can be: a number for one thumb, or one number per
/// thumb, like `vec![20.0, 80.0]` for a range (`number | number[]`).
pub trait SliderValue: Clone + PartialEq + Debug + 'static {
    /// One number per thumb (`valuesOf`).
    fn values(&self) -> Vec<f64>;

    /// A value of this shape from one number per thumb (`shaped`), so
    /// `on_value_change` gets back the shape it was given.
    fn shaped(values: Vec<f64>) -> Self;

    /// The value as the agent bridge reads it.
    fn to_agent(&self) -> AgentValue;

    /// The value an agent's argument gives: a number, or a list of them.
    fn from_agent(args: &AgentArgs, index: usize) -> Result<Self, AgentError>;
}

impl SliderValue for f64 {
    fn values(&self) -> Vec<f64> {
        vec![*self]
    }

    fn shaped(values: Vec<f64>) -> Self {
        values.first().copied().unwrap_or(f64::NAN)
    }

    fn to_agent(&self) -> AgentValue {
        AgentValue::from(*self)
    }

    fn from_agent(args: &AgentArgs, index: usize) -> Result<Self, AgentError> {
        args.number(index)
    }
}

impl SliderValue for Vec<f64> {
    fn values(&self) -> Vec<f64> {
        self.clone()
    }

    fn shaped(values: Vec<f64>) -> Self {
        values
    }

    fn to_agent(&self) -> AgentValue {
        AgentValue::from(self.clone())
    }

    fn from_agent(args: &AgentArgs, index: usize) -> Result<Self, AgentError> {
        if let Ok(number) = args.number(index) {
            return Ok(vec![number]);
        }
        let takes = || AgentError::Takes {
            action: args.action().to_string().into(),
            expected: "a number or a list of numbers",
            given: args.value(index).clone(),
        };
        let texts = args.list(index).map_err(|_| takes())?;
        texts
            .iter()
            .map(|text| text.trim().parse::<f64>().ok().filter(|number| !number.is_nan()))
            .collect::<Option<Vec<f64>>>()
            .filter(|values| !values.is_empty())
            .ok_or_else(takes)
    }
}
