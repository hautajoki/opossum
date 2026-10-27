//! `nodes/jsx`: JSX elements as calls of the JSX factory (`React.createElement(tag, props, ...children)`).

pub mod transform_jsx;
pub mod transform_jsx_attributes;
pub mod transform_jsx_children;
pub mod transform_jsx_tag_name;
