use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcVector2 {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcFootprint {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub position: IpcVector2,
    pub definition_anchor: IpcVector2,
    pub definition_item_samples: Vec<IpcFootprintItemSample>,
    pub definition_item_types: Vec<String>,
    pub rotation: f64,
    pub layer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcFootprintItemSample {
    pub kind: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcBounds {
    pub min: IpcVector2,
    pub max: IpcVector2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcCourtyardPrimitive {
    pub kind: String,
    pub layer: String,
    /// Board-space points in millimetres.  Curves are tessellated for portable
    /// JSON output; the bounds are calculated from the same points.
    pub points: Vec<IpcVector2>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcFootprintCourtyard {
    pub reference: String,
    pub layer: String,
    pub bounds: Option<IpcBounds>,
    pub primitives: Vec<IpcCourtyardPrimitive>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcFootprintPad {
    pub number: String,
    pub position: IpcVector2,
    pub net: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcFootprintText {
    pub reference: String,
    pub kind: String,
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub stroke_width: f64,
    pub rotation: f64,
    pub layer: String,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcTrack {
    pub uuid: String,
    pub net_name: String,
    pub layer: String,
    pub width: f64,
    pub start: IpcVector2,
    pub end: IpcVector2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcVia {
    pub uuid: String,
    pub net_name: String,
    pub position: IpcVector2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcBoardText {
    pub uuid: String,
    pub text: String,
    pub layer: String,
    pub position: IpcVector2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcNet {
    pub name: String,
    pub netcode: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcLayer {
    pub name: String,
    pub id: i32,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcBoardExtents {
    pub min: IpcVector2,
    pub max: IpcVector2,
}
