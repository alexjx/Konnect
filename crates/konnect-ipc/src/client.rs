//! KiCAD 10 IPC API client using NNG + Protocol Buffers.
//!
//! KiCAD 10 exposes an IPC API over NNG (nanomsg-next-gen) using protobuf messages.
//! The transport is NNG req/rep over IPC (Unix sockets / Windows named pipes).
//!
//! Socket path: set by KICAD_API_SOCKET env var when KiCAD launches a plugin,
//! or can be manually specified.
//!
//! Protocol: ApiRequest envelope containing a google.protobuf.Any body → ApiResponse.

use crate::gen::kiapi;
use crate::types::*;
use anyhow::{Context, Result};
// NNG SetOpt trait is brought in scope automatically by the nng crate's prelude
use prost::Message;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

static CLIENT_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("KiCad IPC returned {code}: {message}")]
    ApiStatus { code: String, message: String },
    #[error("KiCad IPC response token does not match the active KiCad session")]
    TokenMismatch,
    #[error("KiCad IPC response was invalid: {0}")]
    InvalidResponse(String),
    #[error("the requested PCB is not open in the connected KiCad instance: {0}")]
    DocumentNotOpen(PathBuf),
    #[error("KiCad may have accepted {command}, but its response was not received: {source}")]
    OutcomeUnknown {
        command: String,
        #[source]
        source: anyhow::Error,
    },
}

/// Converts KiCAD nanometers to millimeters.
fn nm_to_mm(nm: i64) -> f64 {
    nm as f64 / 1_000_000.0
}

/// Map a BoardLayer enum integer back to a KiCAD layer name string.
fn layer_enum_to_name(layer: i32) -> &'static str {
    match kiapi::board::types::BoardLayer::try_from(layer) {
        Ok(l) => match l {
            kiapi::board::types::BoardLayer::BlFCu => "F.Cu",
            kiapi::board::types::BoardLayer::BlBCu => "B.Cu",
            kiapi::board::types::BoardLayer::BlIn1Cu => "In1.Cu",
            kiapi::board::types::BoardLayer::BlIn2Cu => "In2.Cu",
            kiapi::board::types::BoardLayer::BlFSilkS => "F.SilkS",
            kiapi::board::types::BoardLayer::BlBSilkS => "B.SilkS",
            kiapi::board::types::BoardLayer::BlFMask => "F.Mask",
            kiapi::board::types::BoardLayer::BlBMask => "B.Mask",
            kiapi::board::types::BoardLayer::BlFPaste => "F.Paste",
            kiapi::board::types::BoardLayer::BlBPaste => "B.Paste",
            kiapi::board::types::BoardLayer::BlFCrtYd => "F.CrtYd",
            kiapi::board::types::BoardLayer::BlBCrtYd => "B.CrtYd",
            kiapi::board::types::BoardLayer::BlFFab => "F.Fab",
            kiapi::board::types::BoardLayer::BlBFab => "B.Fab",
            kiapi::board::types::BoardLayer::BlDwgsUser => "Dwgs.User",
            kiapi::board::types::BoardLayer::BlEdgeCuts => "Edge.Cuts",
            _ => "Unknown",
        },
        Err(_) => "Unknown",
    }
}

/// Wrap a protobuf message into a prost_types::Any with the correct type_url.
fn pack_any<M: Message>(msg: &M, type_name: &str) -> prost_types::Any {
    let mut buf = Vec::new();
    msg.encode(&mut buf).expect("protobuf encode failed");
    prost_types::Any {
        type_url: format!("type.googleapis.com/{}", type_name),
        value: buf,
    }
}

/// Decode a prost_types::Any into a specific protobuf message type.
fn unpack_any<M: Message + Default>(any: &prost_types::Any) -> Result<M> {
    M::decode(any.value.as_slice()).context("Failed to decode protobuf Any body")
}

fn canonical_pcb_path(path: &Path) -> Result<PathBuf> {
    let is_pcb = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("kicad_pcb"));
    if !is_pcb {
        anyhow::bail!("expected a .kicad_pcb file: {}", path.display());
    }
    std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve PCB path {}", path.display()))
}

fn document_path(doc: &kiapi::common::types::DocumentSpecifier) -> Option<PathBuf> {
    use kiapi::common::types::document_specifier::Identifier;
    let filename = match doc.identifier.as_ref()? {
        Identifier::BoardFilename(filename) => filename,
        _ => return None,
    };
    let project = doc.project.as_ref()?;
    let candidate = PathBuf::from(&project.path).join(filename);
    std::fs::canonicalize(candidate).ok()
}

fn normalized_path(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let value = value.to_ascii_lowercase();
    value
        .trim_start_matches("//?/")
        .trim_end_matches('/')
        .to_string()
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    normalized_path(left) == normalized_path(right)
}

fn update_via_dimensions(
    via: &mut kiapi::board::types::Via,
    drill: Option<f64>,
    pad_size: Option<f64>,
) -> Result<()> {
    let stack = via
        .pad_stack
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("via has no pad stack"))?;
    let drill_properties = stack
        .drill
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("via has no drill properties"))?;
    let current_drill = drill_properties
        .diameter
        .as_ref()
        .map(|diameter| crate::builders::nm_to_mm(diameter.x_nm))
        .ok_or_else(|| anyhow::anyhow!("via has no drill diameter"))?;
    let current_pad_size = stack
        .copper_layers
        .first()
        .and_then(|layer| layer.size.as_ref())
        .map(|size| crate::builders::nm_to_mm(size.x_nm))
        .ok_or_else(|| anyhow::anyhow!("via has no copper diameter"))?;
    let target_drill = drill.unwrap_or(current_drill);
    let target_pad_size = pad_size.unwrap_or(current_pad_size);
    if target_pad_size <= target_drill {
        anyhow::bail!(
            "via pad_size ({target_pad_size}) must be greater than drill ({target_drill})"
        );
    }

    if let Some(drill) = drill {
        drill_properties.diameter = Some(crate::builders::vec2(drill, drill));
    }
    if let Some(pad_size) = pad_size {
        if stack.copper_layers.is_empty() {
            anyhow::bail!("via has no copper layers");
        }
        for layer in &mut stack.copper_layers {
            layer.size = Some(crate::builders::vec2(pad_size, pad_size));
        }
    }
    Ok(())
}

fn has_board_filename(doc: &kiapi::common::types::DocumentSpecifier) -> bool {
    matches!(
        doc.identifier,
        Some(kiapi::common::types::document_specifier::Identifier::BoardFilename(ref name))
            if !name.is_empty()
    )
}

struct IpcSession {
    socket_path: String,
    client_name: String,
    token: Mutex<String>,
    request_gate: Mutex<()>,
}

#[derive(Clone)]
struct OpenBoardDocument {
    specifier: kiapi::common::types::DocumentSpecifier,
    canonical_path: PathBuf,
}

/// A reconnectable KiCad IPC session. Clones share the instance token and
/// request gate; a board-bound clone additionally carries the exact document.
#[derive(Clone)]
pub struct KiCadIpcClient {
    session: Arc<IpcSession>,
    document: Option<OpenBoardDocument>,
}

impl KiCadIpcClient {
    /// Create a client connecting to the given IPC socket path.
    /// If empty, tries KICAD_API_SOCKET environment variable.
    pub fn new(socket_path: impl Into<String>) -> Self {
        let path = socket_path.into();
        let effective_path = if path.is_empty() {
            std::env::var("KICAD_API_SOCKET").unwrap_or_default()
        } else {
            path
        };
        let sequence = CLIENT_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        KiCadIpcClient {
            session: Arc::new(IpcSession {
                socket_path: effective_path,
                client_name: format!("konnect-{}-{}", std::process::id(), sequence),
                token: Mutex::new(std::env::var("KICAD_API_TOKEN").unwrap_or_default()),
                request_gate: Mutex::new(()),
            }),
            document: None,
        }
    }

    /// Send a protobuf command and return the response Any.
    fn send_command(
        &self,
        command: &impl Message,
        type_name: &str,
    ) -> Result<Option<prost_types::Any>> {
        if self.session.socket_path.is_empty() {
            anyhow::bail!(
                "KiCAD IPC socket path not configured. To fix: \
                 (1) in KiCAD, enable Edit > Preferences > Plugins > 'Enable KiCad API' \
                 and copy the listed ipc:// address; \
                 (2) paste it into the 'IPC Socket' field of the Konnect settings dialog \
                 (Tools > External Plugins > Konnect) and save; \
                 (3) restart the AI client so the server rereads settings. \
                 Alternatively set ipc_socket_path in konnect-settings.json or launch \
                 via KiCAD (which sets KICAD_API_SOCKET). \
                 Full guide: https://github.com/mixelpixx/Konnect/blob/main/docs/TROUBLESHOOTING.md"
            );
        }

        // KiCad accepts only one synchronous request at a time. Keep this gate
        // across dial/send/receive, while creating a fresh socket so a timed-out
        // REQ socket is never reused.
        let _request_guard = self
            .session
            .request_gate
            .lock()
            .map_err(|_| anyhow::anyhow!("KiCad IPC request gate was poisoned"))?;
        let token = self
            .session
            .token
            .lock()
            .map_err(|_| anyhow::anyhow!("KiCad IPC token state was poisoned"))?
            .clone();

        let request = kiapi::common::ApiRequest {
            header: Some(kiapi::common::ApiRequestHeader {
                kicad_token: token,
                client_name: self.session.client_name.clone(),
            }),
            message: Some(pack_any(command, type_name)),
        };

        let request_bytes = request.encode_to_vec();
        debug!(
            "[BETA] IPC → {} ({} bytes) to {}",
            type_name,
            request_bytes.len(),
            self.session.socket_path
        );

        // Connect via NNG req0 socket
        let socket =
            nng::Socket::new(nng::Protocol::Req0).context("Failed to create NNG socket")?;

        // Bound every step: a busy or wedged KiCAD must produce an error the
        // tools can surface, never an indefinite hang (the predecessor
        // project's sync/autoroute hangs blocked for >600 s on exactly this).
        // 30 s receive allows slow board operations like zone refills.
        use nng::options::Options;
        socket
            .set_opt::<nng::options::SendTimeout>(Some(std::time::Duration::from_secs(5)))
            .context("Failed to set NNG send timeout")?;
        socket
            .set_opt::<nng::options::RecvTimeout>(Some(std::time::Duration::from_secs(30)))
            .context("Failed to set NNG receive timeout")?;

        // Build the dial URL
        let dial_url = if self.session.socket_path.starts_with("ipc://")
            || self.session.socket_path.starts_with("tcp://")
        {
            self.session.socket_path.clone()
        } else {
            format!("ipc://{}", self.session.socket_path)
        };

        socket
            .dial(&dial_url)
            .with_context(|| format!("Cannot connect to KiCAD IPC at {}", dial_url))?;

        // Send request
        let msg = nng::Message::from(request_bytes.as_slice());
        socket
            .send(msg)
            .map_err(|(_, e)| anyhow::anyhow!("NNG send failed: {}", e))?;

        // Receive response
        let reply = socket.recv().map_err(|e| IpcError::OutcomeUnknown {
            command: type_name.to_string(),
            source: anyhow::anyhow!("NNG recv failed: {}", e),
        })?;

        let response = kiapi::common::ApiResponse::decode(reply.as_slice())
            .context("Failed to decode ApiResponse")?;

        let response_token = response
            .header
            .as_ref()
            .map(|header| header.kicad_token.as_str())
            .unwrap_or_default();
        let mut session_token = self
            .session
            .token
            .lock()
            .map_err(|_| anyhow::anyhow!("KiCad IPC token state was poisoned"))?;
        if session_token.is_empty() {
            if response_token.is_empty() {
                return Err(
                    IpcError::InvalidResponse("missing KiCad instance token".to_string()).into(),
                );
            }
            *session_token = response_token.to_string();
        } else if response_token != session_token.as_str() {
            return Err(IpcError::TokenMismatch.into());
        }

        // Check status
        if let Some(ref status) = response.status {
            let code = status.status();
            if code != kiapi::common::ApiStatusCode::AsOk {
                let msg = if status.error_message.is_empty() {
                    format!("{:?}", code)
                } else {
                    status.error_message.clone()
                };
                debug!("[BETA] IPC ← error: {} ({})", msg, code.as_str_name());
                return Err(IpcError::ApiStatus {
                    code: code.as_str_name().to_string(),
                    message: msg,
                }
                .into());
            }
        } else {
            return Err(IpcError::InvalidResponse("missing status".to_string()).into());
        }

        debug!("[BETA] IPC ← OK");
        Ok(response.message)
    }

    // ─── Public API (same interface as before, tools don't change) ───────

    /// Check if KiCAD is reachable.
    pub fn ping(&self) -> Result<bool> {
        let ping = kiapi::common::commands::Ping {};
        match self.send_command(&ping, "kiapi.common.commands.Ping") {
            Ok(_) => Ok(true),
            Err(e) => {
                warn!("[BETA] Ping failed: {}", e);
                Ok(false)
            }
        }
    }

    /// Get the list of open documents (boards).
    pub fn get_open_documents(&self) -> Result<Vec<kiapi::common::types::DocumentSpecifier>> {
        let cmd = kiapi::common::commands::GetOpenDocuments {
            r#type: kiapi::common::types::DocumentType::DoctypePcb as i32,
        };
        let response_any = self.send_command(&cmd, "kiapi.common.commands.GetOpenDocuments")?;
        if let Some(any) = response_any {
            let resp: kiapi::common::commands::GetOpenDocumentsResponse = unpack_any(&any)?;
            Ok(resp.documents)
        } else {
            Ok(vec![])
        }
    }

    /// Bind future document-scoped calls to one exact, existing PCB path.
    /// KiCad 10 identifies boards by filename on the wire, so the project path
    /// comparison here is a required client-side correctness check.
    pub fn bind_board(&self, board: impl AsRef<Path>) -> Result<Self> {
        let canonical_path = canonical_pcb_path(board.as_ref())?;
        let specifier = self
            .get_open_documents()?
            .into_iter()
            .find(|doc| document_path(doc).is_some_and(|path| paths_equal(&path, &canonical_path)))
            .ok_or_else(|| IpcError::DocumentNotOpen(canonical_path.clone()))?;
        Ok(Self {
            session: Arc::clone(&self.session),
            document: Some(OpenBoardDocument {
                specifier,
                canonical_path,
            }),
        })
    }

    pub fn get_version(&self) -> Result<kiapi::common::types::KiCadVersion> {
        let cmd = kiapi::common::commands::GetVersion {};
        let response_any = self.send_command(&cmd, "kiapi.common.commands.GetVersion")?;
        let any = response_any.ok_or_else(|| {
            IpcError::InvalidResponse("GetVersion response has no body".to_string())
        })?;
        let response: kiapi::common::commands::GetVersionResponse = unpack_any(&any)?;
        response.version.ok_or_else(|| {
            IpcError::InvalidResponse("GetVersion response has no version".to_string()).into()
        })
    }

    /// Get the currently bound PCB document and ensure that exact path is still open.
    fn get_board_document(&self) -> Result<kiapi::common::types::DocumentSpecifier> {
        let bound = self.document.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "PCB command requires an exact document binding; call bind_board(path) first"
            )
        })?;
        self.get_open_documents()?
            .into_iter()
            .find(|doc| {
                document_path(doc).is_some_and(|path| paths_equal(&path, &bound.canonical_path))
            })
            .map(|doc| {
                // Prefer KiCad's current specifier, but preserve the originally
                // resolved one for compatibility with sparse mock responses.
                if !has_board_filename(&doc) {
                    bound.specifier.clone()
                } else {
                    doc
                }
            })
            .ok_or_else(|| IpcError::DocumentNotOpen(bound.canonical_path.clone()).into())
    }

    fn make_header(&self) -> Result<kiapi::common::types::ItemHeader> {
        Ok(kiapi::common::types::ItemHeader {
            document: Some(self.get_board_document()?),
            container: None,
            field_mask: None,
        })
    }

    /// Get all nets on the board.
    pub fn get_nets(&self) -> Result<Vec<IpcNet>> {
        let doc = self.get_board_document()?;
        let cmd = kiapi::board::commands::GetNets {
            board: Some(doc),
            netclass_filter: vec![],
        };
        let response_any = self.send_command(&cmd, "kiapi.board.commands.GetNets")?;
        if let Some(any) = response_any {
            let resp: kiapi::board::commands::NetsResponse = unpack_any(&any)?;
            Ok(resp
                .nets
                .iter()
                .map(|n| IpcNet {
                    name: n.name.clone(),
                    netcode: n.code.as_ref().map(|c| c.value).unwrap_or(0),
                })
                .collect())
        } else {
            Ok(vec![])
        }
    }

    /// Get board items by type.
    pub fn get_items(
        &self,
        item_type: kiapi::common::types::KiCadObjectType,
    ) -> Result<Vec<prost_types::Any>> {
        let header = self.make_header()?;
        let cmd = kiapi::common::commands::GetItems {
            header: Some(header),
            types: vec![item_type as i32],
        };
        let response_any = self.send_command(&cmd, "kiapi.common.commands.GetItems")?;
        if let Some(any) = response_any {
            let resp: kiapi::common::commands::GetItemsResponse = unpack_any(&any)?;
            Ok(resp.items)
        } else {
            Ok(vec![])
        }
    }

    /// List all footprints on the board.
    pub fn list_footprints(&self) -> Result<Vec<IpcFootprint>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        let mut footprints = Vec::new();
        for item in &items {
            if let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice()) {
                let pos = fp.position.as_ref();
                let ref_text = fp
                    .reference_field
                    .as_ref()
                    .and_then(|f| f.text.as_ref())
                    .and_then(|bt| bt.text.as_ref())
                    .map(|t| t.text.clone())
                    .unwrap_or_default();
                let val_text = fp
                    .value_field
                    .as_ref()
                    .and_then(|f| f.text.as_ref())
                    .and_then(|bt| bt.text.as_ref())
                    .map(|t| t.text.clone())
                    .unwrap_or_default();
                let lib_id = fp
                    .definition
                    .as_ref()
                    .and_then(|d| d.id.as_ref())
                    .map(|id| format!("{}:{}", id.library_nickname, id.entry_name))
                    .unwrap_or_default();
                let definition_item_samples = fp
                    .definition
                    .as_ref()
                    .map(|definition| {
                        definition
                            .items
                            .iter()
                            .filter(|item| item.type_url.ends_with("kiapi.board.types.Pad"))
                            .filter_map(|item| {
                                kiapi::board::types::Pad::decode(item.value.as_slice())
                                    .ok()
                                    .and_then(|pad| pad.position)
                                    .map(|position| IpcFootprintItemSample {
                                        kind: "pad".to_string(),
                                        x: nm_to_mm(position.x_nm),
                                        y: nm_to_mm(position.y_nm),
                                    })
                            })
                            .take(4)
                            .collect()
                    })
                    .unwrap_or_default();
                let definition_item_types = fp
                    .definition
                    .as_ref()
                    .map(|definition| {
                        definition
                            .items
                            .iter()
                            .map(|item| item.type_url.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                footprints.push(IpcFootprint {
                    reference: ref_text,
                    value: val_text,
                    footprint: lib_id,
                    position: IpcVector2 {
                        x: pos.map(|p| nm_to_mm(p.x_nm)).unwrap_or(0.0),
                        y: pos.map(|p| nm_to_mm(p.y_nm)).unwrap_or(0.0),
                    },
                    definition_anchor: fp
                        .definition
                        .as_ref()
                        .and_then(|d| d.anchor.as_ref())
                        .map(|p| IpcVector2 {
                            x: nm_to_mm(p.x_nm),
                            y: nm_to_mm(p.y_nm),
                        })
                        .unwrap_or(IpcVector2 { x: 0.0, y: 0.0 }),
                    definition_item_samples,
                    definition_item_types,
                    rotation: fp
                        .orientation
                        .as_ref()
                        .map(|a| a.value_degrees)
                        .unwrap_or(0.0),
                    layer: layer_enum_to_name(fp.layer).to_string(),
                });
            }
        }
        Ok(footprints)
    }

    /// Return live, board-space courtyard geometry for every footprint.
    /// KiCad 10 currently exposes definition graphics already transformed into
    /// board coordinates, so applying the footprint transform again is wrong.
    pub fn list_footprint_courtyards(&self) -> Result<Vec<IpcFootprintCourtyard>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        let mut result = Vec::new();
        for item in items {
            let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let reference = fp
                .reference_field
                .as_ref()
                .and_then(|f| f.text.as_ref())
                .and_then(|t| t.text.as_ref())
                .map(|t| t.text.clone())
                .unwrap_or_default();
            let mut by_layer: std::collections::BTreeMap<String, Vec<IpcCourtyardPrimitive>> =
                Default::default();
            if let Some(definition) = fp.definition {
                for child in definition.items {
                    if !child
                        .type_url
                        .ends_with("kiapi.board.types.BoardGraphicShape")
                    {
                        continue;
                    }
                    let Ok(graphic) =
                        kiapi::board::types::BoardGraphicShape::decode(child.value.as_slice())
                    else {
                        continue;
                    };
                    let layer = layer_enum_to_name(graphic.layer).to_string();
                    if layer != "F.CrtYd" && layer != "B.CrtYd" {
                        continue;
                    }
                    if let Some(shape) = graphic.shape {
                        append_courtyard_shape(
                            &layer,
                            &shape,
                            by_layer.entry(layer.clone()).or_default(),
                        );
                    }
                }
            }
            for (layer, primitives) in by_layer {
                let bounds = bounds_for_primitives(&primitives);
                result.push(IpcFootprintCourtyard {
                    reference: reference.clone(),
                    layer,
                    bounds,
                    primitives,
                });
            }
        }
        Ok(result)
    }

    /// Append a circular graphic to a footprint definition through KiCad IPC.
    /// Coordinates returned in footprint definition items are board-space in
    /// KiCad 10, so the new circle is constructed in the same coordinate space.
    pub fn add_footprint_circle(
        &self,
        reference: &str,
        layer: &str,
        center_x_mm: f64,
        center_y_mm: f64,
        diameter_mm: f64,
        width_mm: f64,
    ) -> Result<()> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in items {
            let mut fp = match kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            {
                Ok(fp) => fp,
                Err(_) => continue,
            };
            let fp_ref = fp
                .reference_field
                .as_ref()
                .and_then(|f| f.text.as_ref())
                .and_then(|t| t.text.as_ref())
                .map(|t| t.text.as_str())
                .unwrap_or("");
            if fp_ref != reference {
                continue;
            }
            let definition = fp
                .definition
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Footprint '{}' has no definition", reference))?;
            let circle = crate::builders::board_circle(
                layer,
                width_mm,
                center_x_mm,
                center_y_mm,
                diameter_mm / 2.0,
            );
            definition.items.push(crate::builders::pack_any(
                &circle,
                "kiapi.board.types.BoardGraphicShape",
            ));
            return self.update_items(vec![crate::builders::pack_any(
                &fp,
                "kiapi.board.types.FootprintInstance",
            )]);
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Create items on the board.
    pub fn create_items(&self, items: Vec<prost_types::Any>) -> Result<()> {
        let requested_count = items.len();
        let header = self.make_header()?;
        let cmd = kiapi::common::commands::CreateItems {
            header: Some(header),
            items,
            container: None,
        };
        let response = self
            .send_command(&cmd, "kiapi.common.commands.CreateItems")?
            .ok_or_else(|| anyhow::anyhow!("CreateItems returned no response"))?;
        let response: kiapi::common::commands::CreateItemsResponse = unpack_any(&response)?;
        if response.status != kiapi::common::types::ItemRequestStatus::IrsOk as i32 {
            anyhow::bail!("CreateItems request failed with status {}", response.status);
        }
        if response.created_items.len() != requested_count {
            anyhow::bail!(
                "CreateItems returned {} item results for {} requested items",
                response.created_items.len(),
                requested_count
            );
        }
        for (index, result) in response.created_items.iter().enumerate() {
            let status = result.status.as_ref().ok_or_else(|| {
                anyhow::anyhow!("CreateItems result {} has no item status", index)
            })?;
            if status.code != kiapi::common::commands::ItemStatusCode::IscOk as i32 {
                anyhow::bail!(
                    "CreateItems item {} failed with status {}: {}",
                    index,
                    status.code,
                    status.error_message
                );
            }
        }
        Ok(())
    }

    /// Update existing items by KIID. Generic wrapper mirroring create_items/delete_items;
    /// each `Any` must be a fully-formed board item with an existing `id` populated.
    pub fn update_items(&self, items: Vec<prost_types::Any>) -> Result<()> {
        let requested_count = items.len();
        let header = self.make_header()?;
        let cmd = kiapi::common::commands::UpdateItems {
            header: Some(header),
            items,
        };
        let response = self
            .send_command(&cmd, "kiapi.common.commands.UpdateItems")?
            .ok_or_else(|| anyhow::anyhow!("UpdateItems returned no response"))?;
        let response: kiapi::common::commands::UpdateItemsResponse = unpack_any(&response)?;
        if response.status != kiapi::common::types::ItemRequestStatus::IrsOk as i32 {
            anyhow::bail!("UpdateItems request failed with status {}", response.status);
        }
        if response.updated_items.len() != requested_count {
            anyhow::bail!(
                "UpdateItems returned {} item results for {} requested items",
                response.updated_items.len(),
                requested_count
            );
        }
        for (index, result) in response.updated_items.iter().enumerate() {
            let status = result.status.as_ref().ok_or_else(|| {
                anyhow::anyhow!("UpdateItems result {} has no item status", index)
            })?;
            if status.code != kiapi::common::commands::ItemStatusCode::IscOk as i32 {
                anyhow::bail!(
                    "UpdateItems item {} failed with status {}: {}",
                    index,
                    status.code,
                    status.error_message
                );
            }
        }
        Ok(())
    }

    /// Parse KiCad S-expression items and create them in the active editor.
    pub fn parse_and_create_items(&self, contents: String) -> Result<()> {
        let doc = self.get_board_document()?;
        let cmd = kiapi::common::commands::ParseAndCreateItemsFromString {
            document: Some(doc),
            contents,
        };
        let response = self
            .send_command(&cmd, "kiapi.common.commands.ParseAndCreateItemsFromString")?
            .ok_or_else(|| anyhow::anyhow!("ParseAndCreateItemsFromString returned no response"))?;
        let response: kiapi::common::commands::CreateItemsResponse = unpack_any(&response)?;
        if response.status != kiapi::common::types::ItemRequestStatus::IrsOk as i32 {
            anyhow::bail!(
                "ParseAndCreateItemsFromString failed with request status {}",
                response.status
            );
        }
        if response.created_items.is_empty() {
            anyhow::bail!("ParseAndCreateItemsFromString created no items");
        }
        for (index, result) in response.created_items.iter().enumerate() {
            let status = result.status.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "ParseAndCreateItemsFromString result {} has no status",
                    index
                )
            })?;
            if status.code != kiapi::common::commands::ItemStatusCode::IscOk as i32 {
                anyhow::bail!(
                    "ParseAndCreateItemsFromString item {} failed with status {}: {}",
                    index,
                    status.code,
                    status.error_message
                );
            }
        }
        Ok(())
    }

    /// Replace all Edge.Cuts shapes with a rounded rectangular outline in one Undo commit.
    pub fn replace_board_outline(
        &self,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        radius: f64,
        width: f64,
    ) -> Result<()> {
        let shapes = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbShape)?;
        let mut old_ids = Vec::new();
        for item in shapes {
            if let Ok(shape) = kiapi::board::types::BoardGraphicShape::decode(item.value.as_slice())
            {
                if shape.layer == kiapi::board::types::BoardLayer::BlEdgeCuts as i32 {
                    if let Some(id) = shape.id {
                        old_ids.push(id.value);
                    }
                }
            }
        }

        let r = radius
            .max(0.0)
            .min((x2 - x1).abs() / 2.0)
            .min((y2 - y1).abs() / 2.0);
        let mut items = Vec::new();
        let mut add_segment = |ax, ay, bx, by| {
            items.push(crate::builders::pack_any(
                &crate::builders::board_segment("Edge.Cuts", width, ax, ay, bx, by),
                "kiapi.board.types.BoardGraphicShape",
            ));
        };
        if r == 0.0 {
            add_segment(x1, y1, x2, y1);
            add_segment(x2, y1, x2, y2);
            add_segment(x2, y2, x1, y2);
            add_segment(x1, y2, x1, y1);
        } else {
            add_segment(x1 + r, y1, x2 - r, y1);
            add_segment(x2, y1 + r, x2, y2 - r);
            add_segment(x2 - r, y2, x1 + r, y2);
            add_segment(x1, y2 - r, x1, y1 + r);
            let q = r / 2.0_f64.sqrt();
            for (sx, sy, mx, my, ex, ey) in [
                (x2 - r, y1, x2 - r + q, y1 + r - q, x2, y1 + r),
                (x2, y2 - r, x2 - r + q, y2 - r + q, x2 - r, y2),
                (x1 + r, y2, x1 + r - q, y2 - r + q, x1, y2 - r),
                (x1, y1 + r, x1 + r - q, y1 + r - q, x1 + r, y1),
            ] {
                items.push(crate::builders::pack_any(
                    &crate::builders::board_arc("Edge.Cuts", width, sx, sy, mx, my, ex, ey),
                    "kiapi.board.types.BoardGraphicShape",
                ));
            }
        }

        let commit = self.begin_commit()?;
        let result = (|| {
            if !old_ids.is_empty() {
                self.delete_items(old_ids)?;
            }
            self.create_items(items)?;
            self.push_commit(&commit, "Replace board outline")
        })();
        if result.is_err() {
            let _ = self.drop_commit(&commit);
        }
        result
    }

    /// Delete items by KIID.
    pub fn delete_items(&self, ids: Vec<String>) -> Result<()> {
        let requested_count = ids.len();
        let header = self.make_header()?;
        let cmd = kiapi::common::commands::DeleteItems {
            header: Some(header),
            item_ids: ids
                .iter()
                .map(|id| kiapi::common::types::Kiid { value: id.clone() })
                .collect(),
        };
        let response = self
            .send_command(&cmd, "kiapi.common.commands.DeleteItems")?
            .ok_or_else(|| anyhow::anyhow!("DeleteItems returned no response"))?;
        let response: kiapi::common::commands::DeleteItemsResponse = unpack_any(&response)?;
        if response.status != kiapi::common::types::ItemRequestStatus::IrsOk as i32 {
            anyhow::bail!("DeleteItems request failed with status {}", response.status);
        }
        // KiCad 10.0.x may report IRS_OK while omitting the optional
        // per-item result array.  In that case the document-level status is
        // the only success indication; requiring one result per item makes
        // valid multi-item deletes (for example replacing Edge.Cuts) fail.
        if !response.deleted_items.is_empty() && response.deleted_items.len() != requested_count {
            anyhow::bail!(
                "DeleteItems returned {} item results for {} requested items",
                response.deleted_items.len(),
                requested_count
            );
        }
        for (index, result) in response.deleted_items.iter().enumerate() {
            if result.status != kiapi::common::commands::ItemDeletionStatus::IdsOk as i32 {
                anyhow::bail!(
                    "DeleteItems item {} failed with status {}",
                    index,
                    result.status
                );
            }
        }
        Ok(())
    }

    /// Refill zones on the board.
    pub fn refill_zones(&self) -> Result<()> {
        let doc = self.get_board_document()?;
        let cmd = kiapi::board::commands::RefillZones {
            board: Some(doc),
            zones: vec![],
        };
        self.send_command(&cmd, "kiapi.board.commands.RefillZones")?;
        Ok(())
    }

    /// Save the open board document.
    pub fn save_board(&self) -> Result<()> {
        let doc = self.get_board_document()?;
        let cmd = kiapi::common::commands::SaveDocument {
            document: Some(doc),
        };
        self.send_command(&cmd, "kiapi.common.commands.SaveDocument")?;
        Ok(())
    }

    /// Begin a commit (undo group).
    pub fn begin_commit(&self) -> Result<String> {
        // BeginCommit is session-scoped in KiCad's API and has no document
        // field. Requiring a valid binding here is our correctness safeguard,
        // not a wire-protocol requirement.
        self.get_board_document()?;
        let cmd = kiapi::common::commands::BeginCommit {};
        let response_any = self.send_command(&cmd, "kiapi.common.commands.BeginCommit")?;
        if let Some(any) = response_any {
            let resp: kiapi::common::commands::BeginCommitResponse = unpack_any(&any)?;
            let id = resp.id.map(|id| id.value).unwrap_or_default();
            if id.is_empty() {
                anyhow::bail!("BeginCommit returned an empty commit id");
            }
            Ok(id)
        } else {
            anyhow::bail!("BeginCommit returned no response")
        }
    }

    /// End a commit (push or drop).
    pub fn end_commit(
        &self,
        commit_id: &str,
        action: kiapi::common::commands::CommitAction,
        message: &str,
    ) -> Result<()> {
        let cmd = kiapi::common::commands::EndCommit {
            id: Some(kiapi::common::types::Kiid {
                value: commit_id.to_string(),
            }),
            action: action as i32,
            message: message.to_string(),
        };
        self.send_command(&cmd, "kiapi.common.commands.EndCommit")?;
        Ok(())
    }

    /// Push (commit) changes.
    pub fn push_commit(&self, commit_id: &str, description: &str) -> Result<()> {
        self.end_commit(
            commit_id,
            kiapi::common::commands::CommitAction::CmaCommit,
            description,
        )
    }

    /// Drop (rollback) changes.
    pub fn drop_commit(&self, commit_id: &str) -> Result<()> {
        self.end_commit(
            commit_id,
            kiapi::common::commands::CommitAction::CmaDrop,
            "",
        )
    }

    // ─── PCB Item Operations (real protobuf implementations) ───────────

    /// Resolve a net name to its net code by querying GetNets.
    pub fn resolve_net_code(&self, net_name: &str) -> Result<i32> {
        let nets = self.get_nets()?;
        nets.iter()
            .find(|n| n.name == net_name)
            .map(|n| n.netcode)
            .ok_or_else(|| anyhow::anyhow!("Net '{}' not found on board", net_name))
    }

    /// Find a footprint by reference and return its IpcFootprint + KIID.
    pub fn get_footprint(&self, reference: &str) -> Result<Option<IpcFootprint>> {
        let footprints = self.list_footprints()?;
        Ok(footprints.into_iter().find(|fp| fp.reference == reference))
    }

    /// Find a footprint's KIID by reference.
    fn find_footprint_kiid(&self, reference: &str) -> Result<String> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in &items {
            if let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice()) {
                let ref_text = fp
                    .reference_field
                    .as_ref()
                    .and_then(|f| f.text.as_ref())
                    .and_then(|bt| bt.text.as_ref())
                    .map(|t| t.text.as_str())
                    .unwrap_or("");
                if ref_text == reference {
                    if let Some(id) = &fp.id {
                        return Ok(id.value.clone());
                    }
                }
            }
        }
        anyhow::bail!("Footprint '{}' not found on board", reference)
    }

    /// Add a track segment to the board.
    #[allow(clippy::too_many_arguments)]
    pub fn add_track(
        &self,
        net_name: &str,
        layer: &str,
        width: f64,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    ) -> Result<()> {
        let net_code = self.resolve_net_code(net_name)?;
        let track = crate::builders::build_track(net_name, net_code, layer, width, x1, y1, x2, y2);
        let any = crate::builders::pack_any(&track, "kiapi.board.types.Track");
        self.create_items(vec![any])?;
        Ok(())
    }

    /// Add a plated through via to the board using KiCAD's native IPC model.
    pub fn add_via(&self, net_name: &str, x: f64, y: f64, drill: f64, pad_size: f64) -> Result<()> {
        let net_code = self.resolve_net_code(net_name)?;
        let via = crate::builders::build_via(net_name, net_code, x, y, drill, pad_size);
        let any = crate::builders::pack_any(&via, "kiapi.board.types.Via");
        self.create_items(vec![any])?;

        // KiCAD may inherit the net from copper under the new via and ignore the
        // requested net in CreateItems. Find the just-created via by position and
        // force its net through UpdateItems so inner planes clear correctly.
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbVia)?;
        for item in items.into_iter().rev() {
            if let Ok(mut created) = kiapi::board::types::Via::decode(item.value.as_slice()) {
                let Some(pos) = created.position.as_ref() else {
                    continue;
                };
                if (crate::builders::nm_to_mm(pos.x_nm) - x).abs() < 0.000_001
                    && (crate::builders::nm_to_mm(pos.y_nm) - y).abs() < 0.000_001
                {
                    created.net = Some(crate::builders::net(net_name, net_code));
                    let updated = crate::builders::pack_any(&created, "kiapi.board.types.Via");
                    let mut header = self.make_header()?;
                    header.field_mask = Some(prost_types::FieldMask {
                        paths: vec!["net".to_string()],
                    });
                    let cmd = kiapi::common::commands::UpdateItems {
                        header: Some(header),
                        items: vec![updated],
                    };
                    self.send_command(&cmd, "kiapi.common.commands.UpdateItems")?;
                    return Ok(());
                }
            }
        }
        anyhow::bail!("Created via at ({x}, {y}) could not be found for net assignment")
    }

    /// Delete a track by UUID.
    pub fn delete_track(&self, uuid: &str) -> Result<()> {
        self.delete_items(vec![uuid.to_string()])
    }

    /// Delete a via by UUID.
    pub fn delete_via(&self, uuid: &str) -> Result<()> {
        self.delete_items(vec![uuid.to_string()])
    }

    /// Update one or more existing vias in place, preserving their UUIDs,
    /// positions, nets, layer spans, locking, and all unrelated properties.
    pub fn modify_vias(
        &self,
        uuids: &[String],
        drill: Option<f64>,
        pad_size: Option<f64>,
    ) -> Result<Vec<IpcVia>> {
        if uuids.is_empty() {
            anyhow::bail!("at least one via UUID is required");
        }
        if drill.is_none() && pad_size.is_none() {
            anyhow::bail!("at least one of drill or pad_size is required");
        }
        if drill.is_some_and(|value| value <= 0.0) {
            anyhow::bail!("via drill must be greater than zero");
        }
        if pad_size.is_some_and(|value| value <= 0.0) {
            anyhow::bail!("via pad_size must be greater than zero");
        }
        if let (Some(drill), Some(pad_size)) = (drill, pad_size) {
            if pad_size <= drill {
                anyhow::bail!("via pad_size must be greater than drill");
            }
        }

        let requested: std::collections::HashSet<&str> = uuids.iter().map(String::as_str).collect();
        if requested.len() != uuids.len() {
            anyhow::bail!("duplicate via UUIDs are not allowed");
        }

        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbVia)?;
        let mut updates = Vec::with_capacity(uuids.len());
        let mut found = std::collections::HashSet::with_capacity(uuids.len());
        for item in items {
            let Ok(mut via) = kiapi::board::types::Via::decode(item.value.as_slice()) else {
                continue;
            };
            let uuid = via
                .id
                .as_ref()
                .map(|id| id.value.clone())
                .unwrap_or_default();
            if !requested.contains(uuid.as_str()) {
                continue;
            }

            update_via_dimensions(&mut via, drill, pad_size)?;
            found.insert(uuid);
            updates.push(crate::builders::pack_any(&via, "kiapi.board.types.Via"));
        }

        let missing: Vec<&str> = uuids
            .iter()
            .map(String::as_str)
            .filter(|uuid| !found.contains(*uuid))
            .collect();
        if !missing.is_empty() {
            anyhow::bail!("via UUIDs not found: {}", missing.join(", "));
        }

        let commit = self.begin_commit()?;
        match self.update_items(updates) {
            Ok(()) => {
                if let Err(error) =
                    self.push_commit(&commit, &format!("Modify {} via dimension(s)", uuids.len()))
                {
                    let _ = self.drop_commit(&commit);
                    return Err(error);
                }
            }
            Err(error) => {
                let _ = self.drop_commit(&commit);
                return Err(error);
            }
        }

        let updated = self.get_vias(None)?;
        let by_uuid: std::collections::HashMap<&str, &IpcVia> =
            updated.iter().map(|via| (via.uuid.as_str(), via)).collect();
        uuids
            .iter()
            .map(|uuid| {
                by_uuid
                    .get(uuid.as_str())
                    .map(|via| (*via).clone())
                    .ok_or_else(|| anyhow::anyhow!("updated via '{}' cannot be verified", uuid))
            })
            .collect()
    }

    /// Query tracks, optionally filtered by net and/or layer.
    pub fn get_tracks(
        &self,
        net_filter: Option<&str>,
        layer_filter: Option<&str>,
    ) -> Result<Vec<IpcTrack>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbTrace)?;
        let mut tracks = Vec::new();
        for item in &items {
            if let Ok(track) = kiapi::board::types::Track::decode(item.value.as_slice()) {
                let net_name = track.net.as_ref().map(|n| n.name.as_str()).unwrap_or("");
                let layer_name = layer_enum_to_name(track.layer);

                // Apply net filter
                if let Some(nf) = net_filter {
                    if net_name != nf {
                        continue;
                    }
                }
                // Apply layer filter
                if let Some(lf) = layer_filter {
                    if layer_name != lf {
                        continue;
                    }
                }

                let start = track.start.as_ref();
                let end = track.end.as_ref();
                tracks.push(IpcTrack {
                    uuid: track
                        .id
                        .as_ref()
                        .map(|id| id.value.clone())
                        .unwrap_or_default(),
                    net_name: net_name.to_string(),
                    layer: layer_name.to_string(),
                    width: track
                        .width
                        .as_ref()
                        .map(|w| crate::builders::nm_to_mm(w.value_nm))
                        .unwrap_or(0.25),
                    start: IpcVector2 {
                        x: start
                            .map(|p| crate::builders::nm_to_mm(p.x_nm))
                            .unwrap_or(0.0),
                        y: start
                            .map(|p| crate::builders::nm_to_mm(p.y_nm))
                            .unwrap_or(0.0),
                    },
                    end: IpcVector2 {
                        x: end
                            .map(|p| crate::builders::nm_to_mm(p.x_nm))
                            .unwrap_or(0.0),
                        y: end
                            .map(|p| crate::builders::nm_to_mm(p.y_nm))
                            .unwrap_or(0.0),
                    },
                });
            }
        }
        Ok(tracks)
    }

    /// Query vias, optionally filtered by net.
    pub fn get_vias(&self, net_filter: Option<&str>) -> Result<Vec<IpcVia>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbVia)?;
        let mut vias = Vec::new();
        for item in &items {
            if let Ok(via) = kiapi::board::types::Via::decode(item.value.as_slice()) {
                let net_name = via.net.as_ref().map(|n| n.name.as_str()).unwrap_or("");
                if let Some(nf) = net_filter {
                    if net_name != nf {
                        continue;
                    }
                }
                let position = via.position.as_ref();
                let pad_stack = via.pad_stack.as_ref();
                let pad_size = pad_stack
                    .and_then(|stack| stack.copper_layers.first())
                    .and_then(|layer| layer.size.as_ref())
                    .map(|size| crate::builders::nm_to_mm(size.x_nm))
                    .unwrap_or(0.0);
                let drill = pad_stack
                    .and_then(|stack| stack.drill.as_ref())
                    .and_then(|drill| drill.diameter.as_ref())
                    .map(|diameter| crate::builders::nm_to_mm(diameter.x_nm))
                    .unwrap_or(0.0);
                let layers = pad_stack
                    .map(|stack| {
                        stack
                            .layers
                            .iter()
                            .map(|layer| layer_enum_to_name(*layer).to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                vias.push(IpcVia {
                    uuid: via
                        .id
                        .as_ref()
                        .map(|id| id.value.clone())
                        .unwrap_or_default(),
                    net_name: net_name.to_string(),
                    position: IpcVector2 {
                        x: position
                            .map(|p| crate::builders::nm_to_mm(p.x_nm))
                            .unwrap_or(0.0),
                        y: position
                            .map(|p| crate::builders::nm_to_mm(p.y_nm))
                            .unwrap_or(0.0),
                    },
                    pad_size,
                    drill,
                    layers,
                    locked: via.locked == kiapi::common::types::LockedState::LsLocked as i32,
                });
            }
        }
        Ok(vias)
    }

    /// Query free-standing board text, optionally filtered by exact text and layer.
    pub fn get_board_texts(
        &self,
        text_filter: Option<&str>,
        layer_filter: Option<&str>,
    ) -> Result<Vec<IpcBoardText>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbText)?;
        let mut texts = Vec::new();
        for item in &items {
            if let Ok(board_text) = kiapi::board::types::BoardText::decode(item.value.as_slice()) {
                let Some(text) = board_text.text.as_ref() else {
                    continue;
                };
                let layer = layer_enum_to_name(board_text.layer);
                if let Some(tf) = text_filter {
                    if text.text != tf {
                        continue;
                    }
                }
                if let Some(lf) = layer_filter {
                    if layer != lf {
                        continue;
                    }
                }
                let position = text.position.as_ref();
                texts.push(IpcBoardText {
                    uuid: board_text
                        .id
                        .as_ref()
                        .map(|id| id.value.clone())
                        .unwrap_or_default(),
                    text: text.text.clone(),
                    layer: layer.to_string(),
                    position: IpcVector2 {
                        x: position
                            .map(|p| crate::builders::nm_to_mm(p.x_nm))
                            .unwrap_or(0.0),
                        y: position
                            .map(|p| crate::builders::nm_to_mm(p.y_nm))
                            .unwrap_or(0.0),
                    },
                });
            }
        }
        Ok(texts)
    }

    /// Delete free-standing board text by UUID.
    pub fn delete_board_text(&self, uuid: &str) -> Result<()> {
        self.delete_items(vec![uuid.to_string()])
    }

    /// Query free-standing board polygon graphics, optionally filtered by layer.
    pub fn get_board_polygons(&self, layer_filter: Option<&str>) -> Result<Vec<IpcBoardPolygon>> {
        use kiapi::common::types::graphic_shape::Geometry;

        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbShape)?;
        let mut polygons = Vec::new();
        for item in items {
            let Ok(graphic) = kiapi::board::types::BoardGraphicShape::decode(item.value.as_slice())
            else {
                continue;
            };
            let layer = layer_enum_to_name(graphic.layer);
            if layer_filter.is_some_and(|filter| filter != layer) {
                continue;
            }
            let Some(shape) = graphic.shape.as_ref() else {
                continue;
            };
            let Some(Geometry::Polygon(polyset)) = shape.geometry.as_ref() else {
                continue;
            };
            let outlines = polyset
                .polygons
                .iter()
                .filter_map(|polygon| polygon.outline.as_ref())
                .map(polyline_points)
                .collect::<Vec<_>>();
            if outlines.is_empty() {
                continue;
            }
            let attributes = shape.attributes.as_ref();
            let filled = attributes
                .and_then(|attrs| attrs.fill.as_ref())
                .is_some_and(|fill| {
                    fill.fill_type == kiapi::common::types::GraphicFillType::GftFilled as i32
                });
            let stroke_width = attributes
                .and_then(|attrs| attrs.stroke.as_ref())
                .and_then(|stroke| stroke.width.as_ref())
                .map(|width| crate::builders::nm_to_mm(width.value_nm))
                .unwrap_or(0.0);
            polygons.push(IpcBoardPolygon {
                uuid: graphic
                    .id
                    .as_ref()
                    .map(|id| id.value.clone())
                    .unwrap_or_default(),
                layer: layer.to_string(),
                filled,
                stroke_width,
                outlines,
            });
        }
        Ok(polygons)
    }

    /// Create one free-standing board polygon through KiCad IPC in an Undo commit.
    pub fn add_board_polygon(
        &self,
        points: &[(f64, f64)],
        layer: &str,
        filled: bool,
        stroke_width: f64,
    ) -> Result<()> {
        if points.len() < 3 {
            anyhow::bail!("Board polygon requires at least 3 points");
        }
        if stroke_width < 0.0 {
            anyhow::bail!("Board polygon stroke width must be non-negative");
        }
        if crate::builders::layer_from_name(layer) == kiapi::board::types::BoardLayer::BlUndefined {
            anyhow::bail!("Unknown board layer '{}'", layer);
        }

        let mut polygon = crate::builders::board_polygon(layer, filled, &[points.to_vec()]);
        let shape = polygon
            .shape
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Polygon builder returned no shape"))?;
        let attributes = shape.attributes.get_or_insert_with(Default::default);
        let stroke = attributes.stroke.get_or_insert_with(Default::default);
        stroke.width = Some(crate::builders::distance(stroke_width));

        let any = crate::builders::pack_any(&polygon, "kiapi.board.types.BoardGraphicShape");
        let commit = self.begin_commit()?;
        let result = (|| {
            self.create_items(vec![any])?;
            self.push_commit(&commit, "Add board polygon")
        })();
        if result.is_err() {
            let _ = self.drop_commit(&commit);
        }
        result
    }

    /// Replace one free-standing board polygon's outline through KiCad IPC.
    /// The item UUID and all unspecified attributes remain unchanged.
    pub fn update_board_polygon(
        &self,
        uuid: &str,
        points: &[(f64, f64)],
        layer: Option<&str>,
        filled: Option<bool>,
        stroke_width: Option<f64>,
    ) -> Result<()> {
        use kiapi::common::types::{
            graphic_shape::Geometry, poly_line_node, GraphicFillAttributes, GraphicFillType,
            PolyLine, PolyLineNode, PolySet, PolygonWithHoles,
        };

        if points.len() < 3 {
            anyhow::bail!("Board polygon requires at least 3 points");
        }

        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbShape)?;
        let mut target = items
            .into_iter()
            .filter_map(|item| {
                kiapi::board::types::BoardGraphicShape::decode(item.value.as_slice()).ok()
            })
            .find(|graphic| graphic.id.as_ref().is_some_and(|id| id.value == uuid))
            .ok_or_else(|| anyhow::anyhow!("Board polygon '{}' not found", uuid))?;

        let shape = target
            .shape
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Board graphic '{}' has no shape", uuid))?;
        if !matches!(shape.geometry, Some(Geometry::Polygon(_))) {
            anyhow::bail!("Board graphic '{}' is not a polygon", uuid);
        }
        shape.geometry = Some(Geometry::Polygon(PolySet {
            polygons: vec![PolygonWithHoles {
                outline: Some(PolyLine {
                    nodes: points
                        .iter()
                        .map(|&(x, y)| PolyLineNode {
                            geometry: Some(poly_line_node::Geometry::Point(crate::builders::vec2(
                                x, y,
                            ))),
                        })
                        .collect(),
                    closed: true,
                }),
                holes: vec![],
            }],
        }));

        if let Some(layer) = layer {
            let value = crate::builders::layer_from_name(layer);
            if value == kiapi::board::types::BoardLayer::BlUndefined {
                anyhow::bail!("Unknown board layer '{}'", layer);
            }
            target.layer = value as i32;
        }
        if filled.is_some() || stroke_width.is_some() {
            let attrs = shape.attributes.get_or_insert_with(Default::default);
            if let Some(filled) = filled {
                let fill = attrs.fill.get_or_insert_with(|| GraphicFillAttributes {
                    fill_type: GraphicFillType::GftUnfilled as i32,
                    color: None,
                });
                fill.fill_type = if filled {
                    GraphicFillType::GftFilled as i32
                } else {
                    GraphicFillType::GftUnfilled as i32
                };
            }
            if let Some(width) = stroke_width {
                let stroke = attrs.stroke.get_or_insert_with(Default::default);
                stroke.width = Some(crate::builders::distance(width));
            }
        }

        let any = crate::builders::pack_any(&target, "kiapi.board.types.BoardGraphicShape");
        let commit = self.begin_commit()?;
        let result = (|| {
            self.update_items(vec![any])?;
            self.push_commit(&commit, "Update board polygon")
        })();
        if result.is_err() {
            let _ = self.drop_commit(&commit);
        }
        result
    }

    /// Move a footprint to a new position.
    pub fn move_footprint(&self, reference: &str, x: f64, y: f64) -> Result<()> {
        // Find the footprint, update position, send UpdateItems
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in &items {
            if let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice()) {
                let ref_text = fp
                    .reference_field
                    .as_ref()
                    .and_then(|f| f.text.as_ref())
                    .and_then(|bt| bt.text.as_ref())
                    .map(|t| t.text.as_str())
                    .unwrap_or("");
                if ref_text == reference {
                    // Follow the KiCad IPC transaction model used by kicad-python:
                    // mutate the complete item returned by GetItems, update it inside
                    // an explicit commit, then push the commit for editor Undo.
                    let mut update = fp.clone();
                    // KiCad 10 returns footprint definition geometry in absolute
                    // board coordinates through GetItems (despite the proto comment
                    // describing Pad.position as footprint-relative).  Move the
                    // complete returned footprint as one rigid body.
                    let old_position = update.position.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("Footprint '{}' has no position", reference)
                    })?;
                    let dx_nm = crate::builders::mm_to_nm(x) - old_position.x_nm;
                    let dy_nm = crate::builders::mm_to_nm(y) - old_position.y_nm;
                    translate_footprint_definition(&mut update, dx_nm, dy_nm)?;
                    update.position = Some(crate::builders::vec2(x, y));
                    let any =
                        crate::builders::pack_any(&update, "kiapi.board.types.FootprintInstance");
                    let commit = self.begin_commit()?;
                    match self.update_items(vec![any]) {
                        Ok(()) => {
                            if let Err(error) =
                                self.push_commit(&commit, &format!("Move footprint {reference}"))
                            {
                                let _ = self.drop_commit(&commit);
                                return Err(error);
                            }
                            return Ok(());
                        }
                        Err(error) => {
                            let _ = self.drop_commit(&commit);
                            return Err(error);
                        }
                    }
                }
            }
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Rotate a footprint to a new angle.
    pub fn rotate_footprint(&self, reference: &str, angle: f64) -> Result<()> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in &items {
            if let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice()) {
                let ref_text = fp
                    .reference_field
                    .as_ref()
                    .and_then(|f| f.text.as_ref())
                    .and_then(|bt| bt.text.as_ref())
                    .map(|t| t.text.as_str())
                    .unwrap_or("");
                if ref_text == reference {
                    let mut update = fp.clone();
                    let center = update
                        .position
                        .as_ref()
                        .ok_or_else(|| {
                            anyhow::anyhow!("Footprint '{}' has no position", reference)
                        })?
                        .clone();
                    let old_angle = update
                        .orientation
                        .as_ref()
                        .map(|a| a.value_degrees)
                        .unwrap_or(0.0);
                    rotate_footprint_definition(
                        &mut update,
                        center.x_nm,
                        center.y_nm,
                        angle - old_angle,
                    )?;
                    update.orientation = Some(kiapi::common::types::Angle {
                        value_degrees: angle,
                    });
                    let any =
                        crate::builders::pack_any(&update, "kiapi.board.types.FootprintInstance");
                    let commit = self.begin_commit()?;
                    match self.update_items(vec![any]) {
                        Ok(()) => {
                            if let Err(error) =
                                self.push_commit(&commit, &format!("Rotate footprint {reference}"))
                            {
                                let _ = self.drop_commit(&commit);
                                return Err(error);
                            }
                            return Ok(());
                        }
                        Err(error) => {
                            let _ = self.drop_commit(&commit);
                            return Err(error);
                        }
                    }
                }
            }
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Move and rotate a footprint in one KiCad IPC transaction.
    pub fn transform_footprint(&self, reference: &str, x: f64, y: f64, angle: f64) -> Result<()> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in &items {
            let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = fp
                .reference_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .map(|text| text.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }

            let mut update = fp.clone();
            let old_position = update
                .position
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Footprint '{}' has no position", reference))?
                .clone();
            let old_angle = update
                .orientation
                .as_ref()
                .map(|orientation| orientation.value_degrees)
                .unwrap_or(0.0);
            rotate_footprint_definition(
                &mut update,
                old_position.x_nm,
                old_position.y_nm,
                angle - old_angle,
            )?;
            let dx_nm = crate::builders::mm_to_nm(x) - old_position.x_nm;
            let dy_nm = crate::builders::mm_to_nm(y) - old_position.y_nm;
            translate_footprint_definition(&mut update, dx_nm, dy_nm)?;
            update.position = Some(crate::builders::vec2(x, y));
            update.orientation = Some(kiapi::common::types::Angle {
                value_degrees: angle,
            });

            let any = crate::builders::pack_any(&update, "kiapi.board.types.FootprintInstance");
            let commit = self.begin_commit()?;
            let result = (|| {
                self.update_items(vec![any])?;
                self.push_commit(&commit, &format!("Transform footprint {reference}"))
            })();
            if result.is_err() {
                let _ = self.drop_commit(&commit);
            }
            return result;
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Set every pad in a footprint to one angle relative to the footprint.
    ///
    /// KiCad IPC exposes pad-stack angles in board coordinates, so convert the
    /// requested footprint-relative angle using the instance orientation.
    pub fn set_footprint_pad_relative_angle(
        &self,
        reference: &str,
        relative_angle: f64,
    ) -> Result<Vec<(String, f64, f64)>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in &items {
            if let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice()) {
                let ref_text = fp
                    .reference_field
                    .as_ref()
                    .and_then(|f| f.text.as_ref())
                    .and_then(|bt| bt.text.as_ref())
                    .map(|t| t.text.as_str())
                    .unwrap_or("");
                if ref_text != reference {
                    continue;
                }

                let mut update = fp.clone();
                let instance_angle = update
                    .orientation
                    .as_ref()
                    .map(|angle| angle.value_degrees)
                    .unwrap_or(0.0);
                let target_angle = (instance_angle + relative_angle).rem_euclid(360.0);
                let mut changed = Vec::new();
                if let Some(definition) = update.definition.as_mut() {
                    for definition_item in &mut definition.items {
                        if !definition_item.type_url.ends_with("kiapi.board.types.Pad") {
                            continue;
                        }
                        let mut pad =
                            kiapi::board::types::Pad::decode(definition_item.value.as_slice())?;
                        let pad_stack = pad.pad_stack.get_or_insert_with(Default::default);
                        let angle = pad_stack.angle.get_or_insert_with(Default::default);
                        let old_angle = angle.value_degrees;
                        angle.value_degrees = target_angle;
                        changed.push((pad.number.clone(), old_angle, target_angle));
                        definition_item.value = pad.encode_to_vec();
                    }
                }

                let any = crate::builders::pack_any(&update, "kiapi.board.types.FootprintInstance");
                let commit = self.begin_commit()?;
                match self.update_items(vec![any]) {
                    Ok(()) => {
                        if let Err(error) = self.push_commit(
                            &commit,
                            &format!("Set footprint pad angles for {reference}"),
                        ) {
                            let _ = self.drop_commit(&commit);
                            return Err(error);
                        }
                        return Ok(changed);
                    }
                    Err(error) => {
                        let _ = self.drop_commit(&commit);
                        return Err(error);
                    }
                }
            }
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Flip a footprint between the front and back board sides using KiCad's
    /// native interactive action. Selection is scoped to the requested
    /// footprint and cleared again after the action.
    pub fn flip_footprint(&self, reference: &str) -> Result<()> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        let mut footprint_id = None;
        for item in &items {
            if let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice()) {
                let ref_text = fp
                    .reference_field
                    .as_ref()
                    .and_then(|field| field.text.as_ref())
                    .and_then(|text| text.text.as_ref())
                    .map(|text| text.text.as_str())
                    .unwrap_or("");
                if ref_text == reference {
                    footprint_id = fp.id.clone();
                    break;
                }
            }
        }

        let footprint_id =
            footprint_id.ok_or_else(|| anyhow::anyhow!("Footprint '{}' not found", reference))?;
        let header = self.make_header()?;
        let clear = kiapi::common::commands::ClearSelection {
            header: Some(header.clone()),
        };
        self.send_command(&clear, "kiapi.common.commands.ClearSelection")?;

        let select = kiapi::common::commands::AddToSelection {
            header: Some(header.clone()),
            items: vec![footprint_id],
        };
        self.send_command(&select, "kiapi.common.commands.AddToSelection")?;

        let flip_result = self.run_action("pcbnew.InteractiveEdit.flip");
        let clear = kiapi::common::commands::ClearSelection {
            header: Some(header),
        };
        let clear_result = self.send_command(&clear, "kiapi.common.commands.ClearSelection");
        flip_result?;
        clear_result?;
        Ok(())
    }

    /// Return footprint pads in board coordinates from the live KiCad document.
    pub fn get_footprint_pads(&self, reference: &str) -> Result<Vec<IpcFootprintPad>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in &items {
            let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = fp
                .reference_field
                .as_ref()
                .and_then(|f| f.text.as_ref())
                .and_then(|t| t.text.as_ref())
                .map(|t| t.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }

            let mut pads = Vec::new();
            if let Some(definition) = fp.definition.as_ref() {
                for item in &definition.items {
                    if !item.type_url.ends_with("kiapi.board.types.Pad") {
                        continue;
                    }
                    let pad = kiapi::board::types::Pad::decode(item.value.as_slice())?;
                    let Some(local) = pad.position.as_ref() else {
                        continue;
                    };
                    pads.push(IpcFootprintPad {
                        number: pad.number,
                        // GetItems currently supplies these in board coordinates.
                        // Returning them unchanged also makes this query a useful
                        // post-mutation verification of the live editor geometry.
                        position: IpcVector2 {
                            x: crate::builders::nm_to_mm(local.x_nm),
                            y: crate::builders::nm_to_mm(local.y_nm),
                        },
                        net: pad.net.as_ref().map(|n| n.name.clone()).unwrap_or_default(),
                    });
                }
            }
            return Ok(pads);
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Atomically reassign selected pads of one footprint to existing board nets.
    ///
    /// This updates only the nested Pad.net fields in the live footprint
    /// instance. Footprint placement, graphics, pads, tracks, vias and zones are
    /// otherwise preserved.
    pub fn set_footprint_pad_nets(
        &self,
        reference: &str,
        pad_nets: &[(String, String)],
    ) -> Result<()> {
        let resolved: std::collections::HashMap<&str, kiapi::board::types::Net> = pad_nets
            .iter()
            .map(|(pad_number, net_name)| {
                Ok((
                    pad_number.as_str(),
                    kiapi::board::types::Net {
                        code: Some(kiapi::board::types::NetCode {
                            value: self.resolve_net_code(net_name)?,
                        }),
                        name: net_name.clone(),
                    },
                ))
            })
            .collect::<Result<_>>()?;

        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in items {
            let Ok(mut footprint) =
                kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = footprint
                .reference_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .map(|text| text.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }

            let definition = footprint
                .definition
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Footprint '{}' has no definition", reference))?;
            let mut changed = std::collections::HashSet::new();
            for item in &mut definition.items {
                if !item.type_url.ends_with("kiapi.board.types.Pad") {
                    continue;
                }
                let mut pad = kiapi::board::types::Pad::decode(item.value.as_slice())?;
                let Some(net) = resolved.get(pad.number.as_str()) else {
                    continue;
                };
                pad.net = Some(net.clone());
                *item = crate::builders::pack_any(&pad, "kiapi.board.types.Pad");
                changed.insert(pad.number);
            }

            let missing: Vec<_> = resolved
                .keys()
                .filter(|pad_number| !changed.contains(**pad_number))
                .copied()
                .collect();
            if !missing.is_empty() {
                anyhow::bail!(
                    "Footprint '{}' does not contain requested pad(s): {}",
                    reference,
                    missing.join(", ")
                );
            }

            let update =
                crate::builders::pack_any(&footprint, "kiapi.board.types.FootprintInstance");
            let commit = self.begin_commit()?;
            match self.update_items(vec![update]) {
                Ok(()) => {
                    if let Err(error) = self.push_commit(
                        &commit,
                        &format!("Reassign footprint pad nets for {reference}"),
                    ) {
                        let _ = self.drop_commit(&commit);
                        return Err(error);
                    }
                    return Ok(());
                }
                Err(error) => {
                    let _ = self.drop_commit(&commit);
                    return Err(error);
                }
            }
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Atomically replace the complete pad layout of one live footprint.
    ///
    /// Numbered pads inherit their existing nets by pad number. Unnumbered
    /// NPTH pads have no net. KiCad 10 currently consumes nested footprint pad
    /// positions in board-absolute coordinates, matching get_footprint_pads().
    pub fn replace_footprint_pad_layout(
        &self,
        reference: &str,
        pad_specs: &[serde_json::Value],
        description: Option<&str>,
    ) -> Result<usize> {
        use kiapi::board::types::{
            BoardLayer, DrillProperties, DrillShape, Pad, PadStack, PadStackLayer, PadStackShape,
            PadStackType, PadType, UnconnectedLayerRemoval,
        };
        use kiapi::common::types::LockedState;

        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in items {
            let Ok(mut footprint) =
                kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = footprint
                .reference_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .map(|text| text.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }

            if let Some(description) = description {
                if let Some(text) = footprint
                    .description_field
                    .as_mut()
                    .and_then(|field| field.text.as_mut())
                    .and_then(|board_text| board_text.text.as_mut())
                {
                    text.text = description.to_string();
                }
            }
            let definition = footprint
                .definition
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Footprint '{}' has no definition", reference))?;
            if let Some(description) = description {
                if let Some(text) = definition
                    .description_field
                    .as_mut()
                    .and_then(|field| field.text.as_mut())
                    .and_then(|board_text| board_text.text.as_mut())
                {
                    text.text = description.to_string();
                }
            }
            let existing_nets: std::collections::HashMap<String, kiapi::board::types::Net> =
                definition
                    .items
                    .iter()
                    .filter(|item| item.type_url.ends_with("kiapi.board.types.Pad"))
                    .filter_map(|item| Pad::decode(item.value.as_slice()).ok())
                    .filter_map(|pad| pad.net.map(|net| (pad.number, net)))
                    .collect();
            definition
                .items
                .retain(|item| !item.type_url.ends_with("kiapi.board.types.Pad"));

            for spec in pad_specs {
                let number = spec["number"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("pad number must be a string"))?
                    .to_string();
                let pad_type = match spec["type"].as_str() {
                    Some("thru_hole") => PadType::PtPth,
                    Some("np_thru_hole") => PadType::PtNpth,
                    other => anyhow::bail!("Unsupported pad type: {:?}", other),
                };
                let pad_shape = match spec["shape"].as_str() {
                    Some("circle") => PadStackShape::PssCircle,
                    Some("oval") => PadStackShape::PssOval,
                    Some("rect") => PadStackShape::PssRectangle,
                    Some("roundrect") => PadStackShape::PssRoundrect,
                    other => anyhow::bail!("Unsupported pad shape: {:?}", other),
                };
                let read = |name: &str| {
                    spec[name]
                        .as_f64()
                        .ok_or_else(|| anyhow::anyhow!("pad {} missing", name))
                };
                let x = read("x")?;
                let y = read("y")?;
                let width = read("width")?;
                let height = read("height")?;
                let drill_width = read("drill_width")?;
                let drill_height = read("drill_height")?;
                let copper = |layer| PadStackLayer {
                    layer,
                    shape: pad_shape as i32,
                    size: Some(crate::builders::vec2(width, height)),
                    ..Default::default()
                };
                let pad = Pad {
                    id: None,
                    locked: LockedState::LsUnlocked as i32,
                    number: number.clone(),
                    net: existing_nets.get(&number).cloned(),
                    r#type: pad_type as i32,
                    pad_stack: Some(PadStack {
                        r#type: PadStackType::PstNormal as i32,
                        layers: vec![BoardLayer::BlFCu as i32, BoardLayer::BlBCu as i32],
                        drill: Some(DrillProperties {
                            start_layer: BoardLayer::BlFCu as i32,
                            end_layer: BoardLayer::BlBCu as i32,
                            diameter: Some(crate::builders::vec2(drill_width, drill_height)),
                            shape: if (drill_width - drill_height).abs() < 1e-9 {
                                DrillShape::DsCircle as i32
                            } else {
                                DrillShape::DsOblong as i32
                            },
                            ..Default::default()
                        }),
                        unconnected_layer_removal: UnconnectedLayerRemoval::UlrKeep as i32,
                        copper_layers: vec![
                            copper(BoardLayer::BlFCu as i32),
                            copper(BoardLayer::BlBCu as i32),
                        ],
                        ..Default::default()
                    }),
                    position: Some(crate::builders::vec2(x, y)),
                    ..Default::default()
                };
                definition
                    .items
                    .push(crate::builders::pack_any(&pad, "kiapi.board.types.Pad"));
            }

            let update =
                crate::builders::pack_any(&footprint, "kiapi.board.types.FootprintInstance");
            let commit = self.begin_commit()?;
            match self.update_items(vec![update]) {
                Ok(()) => {
                    if let Err(error) = self.push_commit(
                        &commit,
                        &format!("Replace footprint pad layout for {reference}"),
                    ) {
                        let _ = self.drop_commit(&commit);
                        return Err(error);
                    }
                    return Ok(pad_specs.len());
                }
                Err(error) => {
                    let _ = self.drop_commit(&commit);
                    return Err(error);
                }
            }
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Atomically replace one rectangular courtyard primitive and optionally one
    /// nested footprint zone outline while preserving the footprint identity and
    /// every unrelated definition item. An empty `zone_points` slice leaves all
    /// nested zones unchanged. Coordinates are board-absolute, matching the
    /// transformed live KiCad 10 footprint definition returned by GetItems.
    #[allow(clippy::too_many_arguments)]
    pub fn update_footprint_mechanical_geometry(
        &self,
        reference: &str,
        zone_index: usize,
        zone_points: &[(f64, f64)],
        zone_layers: &[String],
        courtyard_layer: &str,
        courtyard_index: usize,
        courtyard_x1: f64,
        courtyard_y1: f64,
        courtyard_x2: f64,
        courtyard_y2: f64,
    ) -> Result<()> {
        use kiapi::common::types::{
            graphic_shape::Geometry, poly_line_node, GraphicRectangleAttributes, PolyLine,
            PolyLineNode, PolySet, PolygonWithHoles,
        };

        if !zone_points.is_empty() && zone_points.len() < 3 {
            anyhow::bail!("Footprint zone requires at least 3 points");
        }
        let courtyard_layer_value = crate::builders::layer_from_name(courtyard_layer);
        if courtyard_layer_value == kiapi::board::types::BoardLayer::BlUndefined {
            anyhow::bail!("Unknown courtyard layer '{}'", courtyard_layer);
        }

        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in items {
            let Ok(mut footprint) =
                kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = footprint
                .reference_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .map(|text| text.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }

            let definition = footprint
                .definition
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Footprint '{}' has no definition", reference))?;

            let mut seen_zones = 0usize;
            let mut zone_updated = zone_points.is_empty();
            let mut seen_courtyards = 0usize;
            let mut courtyard_updated = false;
            for definition_item in &mut definition.items {
                if !zone_points.is_empty()
                    && definition_item.type_url.ends_with("kiapi.board.types.Zone")
                {
                    if seen_zones == zone_index {
                        let mut zone =
                            kiapi::board::types::Zone::decode(definition_item.value.as_slice())?;
                        zone.outline = Some(PolySet {
                            polygons: vec![PolygonWithHoles {
                                outline: Some(PolyLine {
                                    nodes: zone_points
                                        .iter()
                                        .map(|&(x, y)| PolyLineNode {
                                            geometry: Some(poly_line_node::Geometry::Point(
                                                crate::builders::vec2(x, y),
                                            )),
                                        })
                                        .collect(),
                                    closed: true,
                                }),
                                holes: vec![],
                            }],
                        });
                        if !zone_layers.is_empty() {
                            let mut replacement_layers = Vec::with_capacity(zone_layers.len());
                            for layer_name in zone_layers {
                                let layer = crate::builders::layer_from_name(layer_name);
                                if layer == kiapi::board::types::BoardLayer::BlUndefined {
                                    anyhow::bail!("Unknown zone layer '{}'", layer_name);
                                }
                                replacement_layers.push(layer as i32);
                            }
                            zone.layers = replacement_layers;
                        }
                        *definition_item =
                            crate::builders::pack_any(&zone, "kiapi.board.types.Zone");
                        zone_updated = true;
                    }
                    seen_zones += 1;
                    continue;
                }

                if !definition_item
                    .type_url
                    .ends_with("kiapi.board.types.BoardGraphicShape")
                {
                    continue;
                }
                let mut graphic = kiapi::board::types::BoardGraphicShape::decode(
                    definition_item.value.as_slice(),
                )?;
                if graphic.layer != courtyard_layer_value as i32 {
                    continue;
                }
                let Some(shape) = graphic.shape.as_mut() else {
                    continue;
                };
                if !matches!(shape.geometry, Some(Geometry::Rectangle(_))) {
                    continue;
                }
                if seen_courtyards == courtyard_index {
                    shape.geometry = Some(Geometry::Rectangle(GraphicRectangleAttributes {
                        top_left: Some(crate::builders::vec2(courtyard_x1, courtyard_y1)),
                        bottom_right: Some(crate::builders::vec2(courtyard_x2, courtyard_y2)),
                        corner_radius: None,
                    }));
                    *definition_item =
                        crate::builders::pack_any(&graphic, "kiapi.board.types.BoardGraphicShape");
                    courtyard_updated = true;
                }
                seen_courtyards += 1;
            }

            if !zone_updated {
                anyhow::bail!(
                    "Footprint '{}' has no nested zone at index {}",
                    reference,
                    zone_index
                );
            }
            if !courtyard_updated {
                anyhow::bail!(
                    "Footprint '{}' has no rectangular {} primitive at index {}",
                    reference,
                    courtyard_layer,
                    courtyard_index
                );
            }

            let update =
                crate::builders::pack_any(&footprint, "kiapi.board.types.FootprintInstance");
            let commit = self.begin_commit()?;
            let result = (|| {
                self.update_items(vec![update])?;
                self.push_commit(
                    &commit,
                    &format!("Update footprint mechanical geometry for {reference}"),
                )
            })();
            if result.is_err() {
                let _ = self.drop_commit(&commit);
            }
            return result;
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Delete all nested zones (including keepouts) from one footprint while
    /// preserving its identity, transform and every non-zone definition item.
    pub fn delete_footprint_nested_zones(&self, reference: &str) -> Result<usize> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in items {
            let Ok(mut footprint) =
                kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = footprint
                .reference_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .map(|text| text.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }

            let definition = footprint
                .definition
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Footprint '{}' has no definition", reference))?;
            let before = definition.items.len();
            definition
                .items
                .retain(|item| !item.type_url.ends_with("kiapi.board.types.Zone"));
            let removed = before - definition.items.len();
            if removed == 0 {
                return Ok(0);
            }

            let update =
                crate::builders::pack_any(&footprint, "kiapi.board.types.FootprintInstance");
            let commit = self.begin_commit()?;
            let result = (|| {
                self.update_items(vec![update])?;
                self.push_commit(
                    &commit,
                    &format!("Delete nested footprint zones from {reference}"),
                )
            })();
            if result.is_err() {
                let _ = self.drop_commit(&commit);
            }
            result?;
            return Ok(removed);
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Return the 3-D model transforms embedded in one live footprint instance.
    pub fn get_footprint_3d_models(&self, reference: &str) -> Result<Vec<IpcFootprint3DModel>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in items {
            let Ok(footprint) =
                kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = footprint
                .reference_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .map(|text| text.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }
            let definition = footprint
                .definition
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Footprint '{}' has no definition", reference))?;
            let mut models = Vec::new();
            for item in &definition.items {
                if !item
                    .type_url
                    .ends_with("kiapi.board.types.Footprint3DModel")
                {
                    continue;
                }
                let model = kiapi::board::types::Footprint3DModel::decode(item.value.as_slice())?;
                let scale = model.scale.unwrap_or_default();
                let rotation = model.rotation.unwrap_or_default();
                let offset = model.offset.unwrap_or_default();
                models.push(IpcFootprint3DModel {
                    filename: model.filename,
                    scale: [scale.x_nm, scale.y_nm, scale.z_nm],
                    rotation: [rotation.x_nm, rotation.y_nm, rotation.z_nm],
                    // KiCad's Footprint3DModel reuses Vector3D but stores model
                    // offsets directly in millimetres, despite the generated
                    // protobuf field names ending in `_nm`.
                    offset_mm: [offset.x_nm, offset.y_nm, offset.z_nm],
                    visible: model.visible,
                    opacity: model.opacity,
                });
            }
            return Ok(models);
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Update only one embedded 3-D model transform in a live footprint.
    pub fn set_footprint_3d_model_transform(
        &self,
        reference: &str,
        model_index: usize,
        offset_mm: [f64; 3],
        rotation: [f64; 3],
    ) -> Result<()> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in items {
            let Ok(mut footprint) =
                kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = footprint
                .reference_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .map(|text| text.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }
            let definition = footprint
                .definition
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("Footprint '{}' has no definition", reference))?;
            let mut current_index = 0usize;
            let mut changed = false;
            for item in &mut definition.items {
                if !item
                    .type_url
                    .ends_with("kiapi.board.types.Footprint3DModel")
                {
                    continue;
                }
                if current_index == model_index {
                    let mut model =
                        kiapi::board::types::Footprint3DModel::decode(item.value.as_slice())?;
                    model.offset = Some(kiapi::common::types::Vector3D {
                        x_nm: offset_mm[0],
                        y_nm: offset_mm[1],
                        z_nm: offset_mm[2],
                    });
                    model.rotation = Some(kiapi::common::types::Vector3D {
                        x_nm: rotation[0],
                        y_nm: rotation[1],
                        z_nm: rotation[2],
                    });
                    *item = crate::builders::pack_any(&model, "kiapi.board.types.Footprint3DModel");
                    changed = true;
                    break;
                }
                current_index += 1;
            }
            if !changed {
                anyhow::bail!(
                    "Footprint '{}' does not contain 3-D model index {}",
                    reference,
                    model_index
                );
            }
            let update =
                crate::builders::pack_any(&footprint, "kiapi.board.types.FootprintInstance");
            let commit = self.begin_commit()?;
            match self.update_items(vec![update]) {
                Ok(()) => {
                    if let Err(error) = self.push_commit(
                        &commit,
                        &format!("Update 3-D model transform for {reference}"),
                    ) {
                        let _ = self.drop_commit(&commit);
                        return Err(error);
                    }
                    return Ok(());
                }
                Err(error) => {
                    let _ = self.drop_commit(&commit);
                    return Err(error);
                }
            }
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Return reference and value text properties for all placed footprints.
    pub fn list_footprint_texts(&self) -> Result<Vec<IpcFootprintText>> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        let mut result = Vec::new();
        for item in &items {
            let Ok(fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let reference = fp
                .reference_field
                .as_ref()
                .and_then(|f| f.text.as_ref())
                .and_then(|bt| bt.text.as_ref())
                .map(|t| t.text.clone())
                .unwrap_or_default();
            for (kind, field) in [
                ("reference", fp.reference_field.as_ref()),
                ("value", fp.value_field.as_ref()),
            ] {
                let Some(field) = field else { continue };
                let Some(board_text) = field.text.as_ref() else {
                    continue;
                };
                let Some(text) = board_text.text.as_ref() else {
                    continue;
                };
                let pos = text.position.as_ref();
                let attrs = text.attributes.as_ref();
                let size = attrs.and_then(|a| a.size.as_ref());
                result.push(IpcFootprintText {
                    reference: reference.clone(),
                    kind: kind.to_string(),
                    text: text.text.clone(),
                    x: pos
                        .map(|p| crate::builders::nm_to_mm(p.x_nm))
                        .unwrap_or(0.0),
                    y: pos
                        .map(|p| crate::builders::nm_to_mm(p.y_nm))
                        .unwrap_or(0.0),
                    width: size
                        .map(|s| crate::builders::nm_to_mm(s.x_nm))
                        .unwrap_or(0.0),
                    height: size
                        .map(|s| crate::builders::nm_to_mm(s.y_nm))
                        .unwrap_or(0.0),
                    stroke_width: attrs
                        .and_then(|a| a.stroke_width.as_ref())
                        .map(|w| crate::builders::nm_to_mm(w.value_nm))
                        .unwrap_or(0.0),
                    rotation: attrs
                        .and_then(|a| a.angle.as_ref())
                        .map(|a| a.value_degrees)
                        .unwrap_or(0.0),
                    layer: layer_enum_to_name(board_text.layer().into()).to_string(),
                    visible: field.visible,
                });
            }
        }
        Ok(result)
    }

    /// Edit a footprint reference field without moving the footprint itself.
    #[allow(clippy::too_many_arguments)]
    pub fn edit_reference_text(
        &self,
        reference: &str,
        x: Option<f64>,
        y: Option<f64>,
        width: Option<f64>,
        height: Option<f64>,
        stroke_width: Option<f64>,
        rotation: Option<f64>,
        visible: Option<bool>,
    ) -> Result<()> {
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbFootprint)?;
        for item in &items {
            let Ok(mut fp) = kiapi::board::types::FootprintInstance::decode(item.value.as_slice())
            else {
                continue;
            };
            let ref_text = fp
                .reference_field
                .as_ref()
                .and_then(|f| f.text.as_ref())
                .and_then(|bt| bt.text.as_ref())
                .map(|t| t.text.as_str())
                .unwrap_or("");
            if ref_text != reference {
                continue;
            }

            let field = fp.reference_field.as_mut().ok_or_else(|| {
                anyhow::anyhow!("Footprint '{}' has no reference field", reference)
            })?;
            if let Some(v) = visible {
                field.visible = v;
            }
            let board_text = field.text.as_mut().ok_or_else(|| {
                anyhow::anyhow!("Footprint '{}' has no reference board text", reference)
            })?;
            let text = board_text.text.as_mut().ok_or_else(|| {
                anyhow::anyhow!("Footprint '{}' has no reference text", reference)
            })?;

            if x.is_some() || y.is_some() {
                let old = text
                    .position
                    .clone()
                    .unwrap_or_else(|| crate::builders::vec2(0.0, 0.0));
                text.position = Some(crate::builders::vec2(
                    x.unwrap_or_else(|| crate::builders::nm_to_mm(old.x_nm)),
                    y.unwrap_or_else(|| crate::builders::nm_to_mm(old.y_nm)),
                ));
            }
            let attrs = text.attributes.get_or_insert_with(Default::default);
            if width.is_some() || height.is_some() {
                let old = attrs
                    .size
                    .clone()
                    .unwrap_or_else(|| crate::builders::vec2(1.0, 1.0));
                attrs.size = Some(crate::builders::vec2(
                    width.unwrap_or_else(|| crate::builders::nm_to_mm(old.x_nm)),
                    height.unwrap_or_else(|| crate::builders::nm_to_mm(old.y_nm)),
                ));
            }
            if let Some(v) = stroke_width {
                attrs.stroke_width = Some(crate::builders::distance(v));
            }
            if let Some(v) = rotation {
                attrs.angle = Some(kiapi::common::types::Angle { value_degrees: v });
            }

            let any = crate::builders::pack_any(&fp, "kiapi.board.types.FootprintInstance");
            self.update_items(vec![any])?;
            return Ok(());
        }
        anyhow::bail!("Footprint '{}' not found", reference)
    }

    /// Delete a footprint by reference.
    pub fn delete_footprint(&self, reference: &str) -> Result<()> {
        let kiid = self.find_footprint_kiid(reference)?;
        self.delete_items(vec![kiid])
    }

    /// Place a footprint — currently requires KiCAD's ParseAndCreateItemsFromString.
    pub fn place_footprint(
        &self,
        lib_id: &str,
        x: f64,
        y: f64,
        rotation: f64,
        layer: &str,
    ) -> Result<IpcFootprint> {
        // KiCAD 10 IPC doesn't have a direct "place footprint from library" command.
        // The CreateItems command requires a fully formed FootprintInstance protobuf,
        // which needs the complete footprint definition (pads, shapes, etc.) from the library.
        // For now, use ParseAndCreateItemsFromString with S-expression format.
        let sexp = format!(
            r#"(footprint "{lib_id}"
  (layer "{layer}")
  (at {x} {y} {rotation})
)"#,
            lib_id = lib_id,
            layer = layer,
            x = crate::builders::mm_to_nm(x) as f64 / 1_000_000.0,
            y = crate::builders::mm_to_nm(y) as f64 / 1_000_000.0,
            rotation = rotation,
        );

        let doc = self.get_board_document()?;
        let cmd = kiapi::common::commands::ParseAndCreateItemsFromString {
            document: Some(doc),
            contents: sexp,
        };
        self.send_command(&cmd, "kiapi.common.commands.ParseAndCreateItemsFromString")?;

        Ok(IpcFootprint {
            reference: String::new(),
            value: String::new(),
            footprint: lib_id.to_string(),
            position: IpcVector2 { x, y },
            definition_anchor: IpcVector2 { x: 0.0, y: 0.0 },
            definition_item_samples: Vec::new(),
            definition_item_types: Vec::new(),
            rotation,
            layer: layer.to_string(),
        })
    }

    /// Get board extents (bounding box of all items).
    pub fn get_board_extents(&self) -> Result<IpcBoardExtents> {
        // Use GetBoundingBox with no specific items = board extents
        let header = self.make_header()?;
        let cmd = kiapi::common::commands::GetBoundingBox {
            header: Some(header),
            items: vec![], // empty = all items
            mode: kiapi::common::commands::BoundingBoxMode::BbmItemOnly as i32,
        };
        let resp_any = self.send_command(&cmd, "kiapi.common.commands.GetBoundingBox")?;
        if let Some(any) = resp_any {
            let resp: kiapi::common::commands::GetBoundingBoxResponse = unpack_any(&any)?;
            if let Some(bbox) = resp.boxes.first() {
                let pos = bbox.position.as_ref();
                let size = bbox.size.as_ref();
                return Ok(IpcBoardExtents {
                    min: IpcVector2 {
                        x: pos
                            .map(|p| crate::builders::nm_to_mm(p.x_nm))
                            .unwrap_or(0.0),
                        y: pos
                            .map(|p| crate::builders::nm_to_mm(p.y_nm))
                            .unwrap_or(0.0),
                    },
                    max: IpcVector2 {
                        x: pos
                            .map(|p| crate::builders::nm_to_mm(p.x_nm))
                            .unwrap_or(0.0)
                            + size
                                .map(|s| crate::builders::nm_to_mm(s.x_nm))
                                .unwrap_or(0.0),
                        y: pos
                            .map(|p| crate::builders::nm_to_mm(p.y_nm))
                            .unwrap_or(0.0)
                            + size
                                .map(|s| crate::builders::nm_to_mm(s.y_nm))
                                .unwrap_or(0.0),
                    },
                });
            }
        }
        anyhow::bail!("No bounding box returned from KiCAD")
    }

    /// Get enabled layers.
    pub fn get_layers(&self) -> Result<Vec<IpcLayer>> {
        let doc = self.get_board_document()?;
        let cmd = kiapi::board::commands::GetBoardEnabledLayers { board: Some(doc) };
        let resp_any = self.send_command(&cmd, "kiapi.board.commands.GetBoardEnabledLayers")?;
        if let Some(any) = resp_any {
            let resp: kiapi::board::commands::BoardEnabledLayersResponse = unpack_any(&any)?;
            let layers = resp
                .layers
                .iter()
                .map(|&l| {
                    let bl = kiapi::board::types::BoardLayer::try_from(l)
                        .unwrap_or(kiapi::board::types::BoardLayer::BlUndefined);
                    IpcLayer {
                        name: bl
                            .as_str_name()
                            .trim_start_matches("BL_")
                            .replace('_', ".")
                            .to_string(),
                        id: l,
                        kind: String::new(),
                    }
                })
                .collect();
            Ok(layers)
        } else {
            Ok(vec![])
        }
    }

    /// Run an arbitrary tool action in KiCAD (e.g. to trigger a refresh).
    pub fn run_action(&self, action: &str) -> Result<()> {
        let cmd = kiapi::common::commands::RunAction {
            action: action.to_string(),
        };
        self.send_command(&cmd, "kiapi.common.commands.RunAction")?;
        Ok(())
    }
}

fn point(v: &kiapi::common::types::Vector2) -> IpcVector2 {
    IpcVector2 {
        x: nm_to_mm(v.x_nm),
        y: nm_to_mm(v.y_nm),
    }
}

fn tessellate_arc(a: &kiapi::common::types::ArcStartMidEnd) -> Vec<IpcVector2> {
    let (Some(s), Some(m), Some(e)) = (&a.start, &a.mid, &a.end) else {
        return vec![];
    };
    let (x1, y1, x2, y2, x3, y3) = (
        s.x_nm as f64,
        s.y_nm as f64,
        m.x_nm as f64,
        m.y_nm as f64,
        e.x_nm as f64,
        e.y_nm as f64,
    );
    let d = 2.0 * (x1 * (y2 - y3) + x2 * (y3 - y1) + x3 * (y1 - y2));
    if d.abs() < 1.0 {
        return vec![point(s), point(m), point(e)];
    }
    let ux = ((x1 * x1 + y1 * y1) * (y2 - y3)
        + (x2 * x2 + y2 * y2) * (y3 - y1)
        + (x3 * x3 + y3 * y3) * (y1 - y2))
        / d;
    let uy = ((x1 * x1 + y1 * y1) * (x3 - x2)
        + (x2 * x2 + y2 * y2) * (x1 - x3)
        + (x3 * x3 + y3 * y3) * (x2 - x1))
        / d;
    let start = (y1 - uy).atan2(x1 - ux);
    let mid = (y2 - uy).atan2(x2 - ux);
    let end = (y3 - uy).atan2(x3 - ux);
    let tau = std::f64::consts::TAU;
    let ccw = (end - start).rem_euclid(tau);
    let mid_ccw = (mid - start).rem_euclid(tau);
    let sweep = if mid_ccw <= ccw { ccw } else { ccw - tau };
    let radius = ((x1 - ux).powi(2) + (y1 - uy).powi(2)).sqrt();
    let steps = ((sweep.abs() / std::f64::consts::FRAC_PI_2) * 16.0)
        .ceil()
        .max(2.0) as usize;
    let mut points: Vec<_> = (0..=steps)
        .map(|i| {
            let t = start + sweep * i as f64 / steps as f64;
            IpcVector2 {
                x: (ux + radius * t.cos()) / 1e6,
                y: (uy + radius * t.sin()) / 1e6,
            }
        })
        .collect();
    // Include exact cardinal extrema so the returned AABB remains exact rather
    // than being slightly smaller than the curve's true bounds.
    for t in [
        0.0,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    ] {
        let delta = if sweep >= 0.0 {
            (t - start).rem_euclid(tau)
        } else {
            -((start - t).rem_euclid(tau))
        };
        if (sweep >= 0.0 && delta <= sweep) || (sweep < 0.0 && delta >= sweep) {
            points.push(IpcVector2 {
                x: (ux + radius * t.cos()) / 1e6,
                y: (uy + radius * t.sin()) / 1e6,
            });
        }
    }
    points
}

fn polyline_points(line: &kiapi::common::types::PolyLine) -> Vec<IpcVector2> {
    use kiapi::common::types::poly_line_node::Geometry;
    line.nodes
        .iter()
        .flat_map(|n| match &n.geometry {
            Some(Geometry::Point(v)) => vec![point(v)],
            Some(Geometry::Arc(a)) => tessellate_arc(a),
            None => vec![],
        })
        .collect()
}

fn append_courtyard_shape(
    layer: &str,
    shape: &kiapi::common::types::GraphicShape,
    out: &mut Vec<IpcCourtyardPrimitive>,
) {
    use kiapi::common::types::graphic_shape::Geometry;
    let mut push = |kind: &str, points: Vec<IpcVector2>| {
        if !points.is_empty() {
            out.push(IpcCourtyardPrimitive {
                kind: kind.into(),
                layer: layer.into(),
                points,
            })
        }
    };
    match &shape.geometry {
        Some(Geometry::Segment(v)) => {
            if let (Some(a), Some(b)) = (&v.start, &v.end) {
                push("segment", vec![point(a), point(b)])
            }
        }
        Some(Geometry::Rectangle(v)) => {
            if let (Some(a), Some(b)) = (&v.top_left, &v.bottom_right) {
                let (a, b) = (point(a), point(b));
                push(
                    "rectangle",
                    vec![
                        IpcVector2 { x: a.x, y: a.y },
                        IpcVector2 { x: b.x, y: a.y },
                        IpcVector2 { x: b.x, y: b.y },
                        IpcVector2 { x: a.x, y: b.y },
                    ],
                )
            }
        }
        Some(Geometry::Arc(v)) => push(
            "arc",
            tessellate_arc(&kiapi::common::types::ArcStartMidEnd {
                start: v.start.clone(),
                mid: v.mid.clone(),
                end: v.end.clone(),
            }),
        ),
        Some(Geometry::Circle(v)) => {
            if let (Some(c), Some(rp)) = (&v.center, &v.radius_point) {
                let (c, rp) = (point(c), point(rp));
                let r = ((rp.x - c.x).powi(2) + (rp.y - c.y).powi(2)).sqrt();
                push(
                    "circle",
                    (0..64)
                        .map(|i| {
                            let t = std::f64::consts::TAU * i as f64 / 64.0;
                            IpcVector2 {
                                x: c.x + r * t.cos(),
                                y: c.y + r * t.sin(),
                            }
                        })
                        .collect(),
                )
            }
        }
        Some(Geometry::Polygon(v)) => {
            for p in &v.polygons {
                if let Some(line) = &p.outline {
                    push("polygon", polyline_points(line))
                }
            }
        }
        Some(Geometry::Bezier(v)) => {
            let pts = [&v.start, &v.control1, &v.control2, &v.end]
                .into_iter()
                .filter_map(|p| p.as_ref())
                .map(point)
                .collect();
            push("bezier", pts)
        }
        None => {}
    }
}

fn bounds_for_primitives(items: &[IpcCourtyardPrimitive]) -> Option<IpcBounds> {
    let mut it = items.iter().flat_map(|p| p.points.iter());
    let first = it.next()?;
    let (mut minx, mut miny, mut maxx, mut maxy) = (first.x, first.y, first.x, first.y);
    for p in it {
        minx = minx.min(p.x);
        miny = miny.min(p.y);
        maxx = maxx.max(p.x);
        maxy = maxy.max(p.y)
    }
    Some(IpcBounds {
        min: IpcVector2 { x: minx, y: miny },
        max: IpcVector2 { x: maxx, y: maxy },
    })
}

#[cfg(test)]
mod courtyard_geometry_tests {
    use super::*;

    #[test]
    fn user_drawings_layer_has_canonical_kicad_name() {
        assert_eq!(
            layer_enum_to_name(kiapi::board::types::BoardLayer::BlDwgsUser as i32),
            "Dwgs.User"
        );
    }

    #[test]
    fn absolute_rectangle_bounds_are_not_transformed_again() {
        use kiapi::common::types::{
            graphic_shape::Geometry, GraphicRectangleAttributes, GraphicShape, Vector2,
        };
        let shape = GraphicShape {
            geometry: Some(Geometry::Rectangle(GraphicRectangleAttributes {
                top_left: Some(Vector2 {
                    x_nm: 10_000_000,
                    y_nm: 20_000_000,
                }),
                bottom_right: Some(Vector2 {
                    x_nm: 14_000_000,
                    y_nm: 23_000_000,
                }),
                corner_radius: None,
            })),
            ..Default::default()
        };
        let mut primitives = vec![];
        append_courtyard_shape("F.CrtYd", &shape, &mut primitives);
        let bounds = bounds_for_primitives(&primitives).unwrap();
        assert_eq!(
            (bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y),
            (10.0, 20.0, 14.0, 23.0)
        );
    }
}

#[cfg(test)]
mod via_dimension_tests {
    use super::*;

    #[test]
    fn update_via_dimensions_preserves_identity_and_connectivity() {
        let mut via = crate::builders::build_via("GND", 1, 12.5, 24.5, 0.2, 0.5);
        via.id = Some(kiapi::common::types::Kiid {
            value: "test-via-uuid".to_string(),
        });
        via.locked = kiapi::common::types::LockedState::LsLocked as i32;
        let original_position = via.position.clone();
        let original_net = via.net.clone();
        let original_layers = via.pad_stack.as_ref().unwrap().layers.clone();

        update_via_dimensions(&mut via, Some(0.3), Some(0.6)).unwrap();

        assert_eq!(via.id.as_ref().unwrap().value, "test-via-uuid");
        assert_eq!(via.position, original_position);
        assert_eq!(via.net, original_net);
        assert_eq!(via.pad_stack.as_ref().unwrap().layers, original_layers);
        assert_eq!(
            via.locked,
            kiapi::common::types::LockedState::LsLocked as i32
        );
        let stack = via.pad_stack.as_ref().unwrap();
        assert!(
            (crate::builders::nm_to_mm(
                stack
                    .drill
                    .as_ref()
                    .unwrap()
                    .diameter
                    .as_ref()
                    .unwrap()
                    .x_nm
            ) - 0.3)
                .abs()
                < 1e-9
        );
        for layer in &stack.copper_layers {
            assert!(
                (crate::builders::nm_to_mm(layer.size.as_ref().unwrap().x_nm) - 0.6).abs() < 1e-9
            );
        }
    }

    #[test]
    fn update_via_dimensions_rejects_invalid_annular_geometry() {
        let mut via = crate::builders::build_via("GND", 1, 12.5, 24.5, 0.3, 0.6);
        let error = update_via_dimensions(&mut via, Some(0.6), None).unwrap_err();
        assert!(error.to_string().contains("must be greater than drill"));
    }
}

#[cfg(test)]
mod footprint_transform_tests {
    use super::*;
    use prost::Message;

    #[test]
    fn absolute_definition_pad_tracks_translation() {
        let mut fp = footprint_with_absolute_pad(45.0, 40.0, 43.8625, 39.05, 0.0);
        translate_footprint_definition(
            &mut fp,
            crate::builders::mm_to_nm(2.0),
            crate::builders::mm_to_nm(-3.5),
        )
        .unwrap();
        let pad = first_pad(&fp);
        let p = pad.position.unwrap();
        assert!((crate::builders::nm_to_mm(p.x_nm) - 45.8625).abs() < 1e-9);
        assert!((crate::builders::nm_to_mm(p.y_nm) - 35.55).abs() < 1e-9);
    }

    #[test]
    fn absolute_definition_pad_tracks_rotation_about_instance_origin() {
        let mut fp = footprint_with_absolute_pad(45.0, 40.0, 43.0, 39.0, 15.0);
        rotate_footprint_definition(
            &mut fp,
            crate::builders::mm_to_nm(45.0),
            crate::builders::mm_to_nm(40.0),
            90.0,
        )
        .unwrap();
        let p = first_pad(&fp).position.unwrap();
        assert!((crate::builders::nm_to_mm(p.x_nm) - 46.0).abs() < 1e-9);
        assert!((crate::builders::nm_to_mm(p.y_nm) - 38.0).abs() < 1e-9);
        let angle = first_pad(&fp)
            .pad_stack
            .unwrap()
            .angle
            .unwrap()
            .value_degrees;
        assert!((angle - 105.0).abs() < 1e-9);
    }

    #[test]
    fn absolute_definition_zone_tracks_translation() {
        let mut fp = footprint_with_absolute_zone(45.0, 40.0, 43.0, 39.0);
        translate_footprint_definition(
            &mut fp,
            crate::builders::mm_to_nm(2.0),
            crate::builders::mm_to_nm(-3.5),
        )
        .unwrap();
        let p = first_zone_outline_point(&fp);
        assert!((crate::builders::nm_to_mm(p.x_nm) - 45.0).abs() < 1e-9);
        assert!((crate::builders::nm_to_mm(p.y_nm) - 35.5).abs() < 1e-9);
    }

    #[test]
    fn absolute_definition_zone_tracks_rotation_about_instance_origin() {
        let mut fp = footprint_with_absolute_zone(45.0, 40.0, 43.0, 39.0);
        rotate_footprint_definition(
            &mut fp,
            crate::builders::mm_to_nm(45.0),
            crate::builders::mm_to_nm(40.0),
            90.0,
        )
        .unwrap();
        let p = first_zone_outline_point(&fp);
        assert!((crate::builders::nm_to_mm(p.x_nm) - 46.0).abs() < 1e-9);
        assert!((crate::builders::nm_to_mm(p.y_nm) - 38.0).abs() < 1e-9);
    }

    fn footprint_with_absolute_pad(
        fp_x: f64,
        fp_y: f64,
        pad_x: f64,
        pad_y: f64,
        pad_angle: f64,
    ) -> kiapi::board::types::FootprintInstance {
        let pad = kiapi::board::types::Pad {
            position: Some(crate::builders::vec2(pad_x, pad_y)),
            pad_stack: Some(kiapi::board::types::PadStack {
                angle: Some(kiapi::common::types::Angle {
                    value_degrees: pad_angle,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        kiapi::board::types::FootprintInstance {
            position: Some(crate::builders::vec2(fp_x, fp_y)),
            definition: Some(kiapi::board::types::Footprint {
                items: vec![prost_types::Any {
                    type_url: "type.googleapis.com/kiapi.board.types.Pad".to_string(),
                    value: pad.encode_to_vec(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn first_pad(fp: &kiapi::board::types::FootprintInstance) -> kiapi::board::types::Pad {
        let item = &fp.definition.as_ref().unwrap().items[0];
        kiapi::board::types::Pad::decode(item.value.as_slice()).unwrap()
    }

    fn footprint_with_absolute_zone(
        fp_x: f64,
        fp_y: f64,
        zone_x: f64,
        zone_y: f64,
    ) -> kiapi::board::types::FootprintInstance {
        use kiapi::common::types::{
            poly_line_node, PolyLine, PolyLineNode, PolySet, PolygonWithHoles,
        };
        let zone = kiapi::board::types::Zone {
            outline: Some(PolySet {
                polygons: vec![PolygonWithHoles {
                    outline: Some(PolyLine {
                        nodes: vec![PolyLineNode {
                            geometry: Some(poly_line_node::Geometry::Point(crate::builders::vec2(
                                zone_x, zone_y,
                            ))),
                        }],
                        closed: true,
                    }),
                    holes: vec![],
                }],
            }),
            ..Default::default()
        };
        kiapi::board::types::FootprintInstance {
            position: Some(crate::builders::vec2(fp_x, fp_y)),
            definition: Some(kiapi::board::types::Footprint {
                items: vec![prost_types::Any {
                    type_url: "type.googleapis.com/kiapi.board.types.Zone".to_string(),
                    value: zone.encode_to_vec(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn first_zone_outline_point(
        fp: &kiapi::board::types::FootprintInstance,
    ) -> kiapi::common::types::Vector2 {
        use kiapi::common::types::poly_line_node;
        let item = &fp.definition.as_ref().unwrap().items[0];
        let zone = kiapi::board::types::Zone::decode(item.value.as_slice()).unwrap();
        let outline = zone.outline.unwrap();
        let node = &outline.polygons[0].outline.as_ref().unwrap().nodes[0];
        match node.geometry.as_ref().unwrap() {
            poly_line_node::Geometry::Point(point) => *point,
            poly_line_node::Geometry::Arc(_) => panic!("expected point"),
        }
    }
}

fn rotate_vector(
    vector: &mut Option<kiapi::common::types::Vector2>,
    cx: i64,
    cy: i64,
    degrees: f64,
) {
    if let Some(point) = vector {
        let radians = degrees.to_radians();
        let (sin, cos) = radians.sin_cos();
        let dx = (point.x_nm - cx) as f64;
        let dy = (point.y_nm - cy) as f64;
        point.x_nm = cx + (dx * cos - dy * sin).round() as i64;
        point.y_nm = cy + (dx * sin + dy * cos).round() as i64;
    }
}

fn rotate_polyline(line: &mut kiapi::common::types::PolyLine, cx: i64, cy: i64, degrees: f64) {
    use kiapi::common::types::poly_line_node::Geometry;
    for node in &mut line.nodes {
        match &mut node.geometry {
            Some(Geometry::Point(point)) => {
                let mut p = Some(point.clone());
                rotate_vector(&mut p, cx, cy, degrees);
                *point = p.unwrap();
            }
            Some(Geometry::Arc(arc)) => {
                rotate_vector(&mut arc.start, cx, cy, degrees);
                rotate_vector(&mut arc.mid, cx, cy, degrees);
                rotate_vector(&mut arc.end, cx, cy, degrees);
            }
            None => {}
        }
    }
}

fn rotate_polyset(polyset: &mut kiapi::common::types::PolySet, cx: i64, cy: i64, degrees: f64) {
    for polygon in &mut polyset.polygons {
        if let Some(line) = &mut polygon.outline {
            rotate_polyline(line, cx, cy, degrees);
        }
        for line in &mut polygon.holes {
            rotate_polyline(line, cx, cy, degrees);
        }
    }
}

fn rotate_zone(zone: &mut kiapi::board::types::Zone, cx: i64, cy: i64, degrees: f64) {
    if let Some(outline) = &mut zone.outline {
        rotate_polyset(outline, cx, cy, degrees);
    }
    for filled in &mut zone.filled_polygons {
        if let Some(shapes) = &mut filled.shapes {
            rotate_polyset(shapes, cx, cy, degrees);
        }
    }
    for layer in &mut zone.layer_properties {
        rotate_vector(&mut layer.hatching_offset, cx, cy, degrees);
    }
}

fn rotate_graphic_shape(
    shape: &mut kiapi::common::types::GraphicShape,
    cx: i64,
    cy: i64,
    degrees: f64,
) {
    use kiapi::common::types::graphic_shape::Geometry;
    match &mut shape.geometry {
        Some(Geometry::Segment(v)) => {
            rotate_vector(&mut v.start, cx, cy, degrees);
            rotate_vector(&mut v.end, cx, cy, degrees);
        }
        Some(Geometry::Rectangle(v)) => {
            rotate_vector(&mut v.top_left, cx, cy, degrees);
            rotate_vector(&mut v.bottom_right, cx, cy, degrees);
        }
        Some(Geometry::Arc(v)) => {
            rotate_vector(&mut v.start, cx, cy, degrees);
            rotate_vector(&mut v.mid, cx, cy, degrees);
            rotate_vector(&mut v.end, cx, cy, degrees);
        }
        Some(Geometry::Circle(v)) => {
            rotate_vector(&mut v.center, cx, cy, degrees);
            rotate_vector(&mut v.radius_point, cx, cy, degrees);
        }
        Some(Geometry::Polygon(v)) => {
            for polygon in &mut v.polygons {
                if let Some(line) = &mut polygon.outline {
                    rotate_polyline(line, cx, cy, degrees);
                }
                for line in &mut polygon.holes {
                    rotate_polyline(line, cx, cy, degrees);
                }
            }
        }
        Some(Geometry::Bezier(v)) => {
            rotate_vector(&mut v.start, cx, cy, degrees);
            rotate_vector(&mut v.control1, cx, cy, degrees);
            rotate_vector(&mut v.control2, cx, cy, degrees);
            rotate_vector(&mut v.end, cx, cy, degrees);
        }
        None => {}
    }
}

fn rotate_field(field: &mut Option<kiapi::board::types::Field>, cx: i64, cy: i64, degrees: f64) {
    if let Some(text) = field
        .as_mut()
        .and_then(|f| f.text.as_mut())
        .and_then(|t| t.text.as_mut())
    {
        rotate_vector(&mut text.position, cx, cy, degrees);
    }
}

fn rotate_footprint_definition(
    footprint: &mut kiapi::board::types::FootprintInstance,
    cx: i64,
    cy: i64,
    degrees: f64,
) -> Result<()> {
    for field in [
        &mut footprint.reference_field,
        &mut footprint.value_field,
        &mut footprint.datasheet_field,
        &mut footprint.description_field,
    ] {
        rotate_field(field, cx, cy, degrees);
    }
    let Some(definition) = footprint.definition.as_mut() else {
        return Ok(());
    };
    for field in [
        &mut definition.reference_field,
        &mut definition.value_field,
        &mut definition.datasheet_field,
        &mut definition.description_field,
    ] {
        rotate_field(field, cx, cy, degrees);
    }
    for item in &mut definition.items {
        if item.type_url.ends_with("kiapi.board.types.Pad") {
            let mut v = kiapi::board::types::Pad::decode(item.value.as_slice())?;
            rotate_vector(&mut v.position, cx, cy, degrees);
            if let Some(pad_stack) = &mut v.pad_stack {
                let angle = pad_stack.angle.get_or_insert_with(Default::default);
                angle.value_degrees += degrees;
            }
            item.value = v.encode_to_vec();
        } else if item
            .type_url
            .ends_with("kiapi.board.types.BoardGraphicShape")
        {
            let mut v = kiapi::board::types::BoardGraphicShape::decode(item.value.as_slice())?;
            if let Some(shape) = &mut v.shape {
                rotate_graphic_shape(shape, cx, cy, degrees);
            }
            item.value = v.encode_to_vec();
        } else if item.type_url.ends_with("kiapi.board.types.BoardText") {
            let mut v = kiapi::board::types::BoardText::decode(item.value.as_slice())?;
            if let Some(text) = &mut v.text {
                rotate_vector(&mut text.position, cx, cy, degrees);
            }
            item.value = v.encode_to_vec();
        } else if item.type_url.ends_with("kiapi.board.types.Zone") {
            let mut v = kiapi::board::types::Zone::decode(item.value.as_slice())?;
            rotate_zone(&mut v, cx, cy, degrees);
            item.value = v.encode_to_vec();
        }
    }
    Ok(())
}

fn translate_vector(vector: &mut Option<kiapi::common::types::Vector2>, dx_nm: i64, dy_nm: i64) {
    if let Some(vector) = vector {
        vector.x_nm += dx_nm;
        vector.y_nm += dy_nm;
    }
}

impl KiCadIpcClient {
    /// Add an NPTH mounting-hole footprint through a normal KiCad IPC commit.
    pub fn add_mounting_hole(&self, reference: &str, x: f64, y: f64, drill: f64) -> Result<()> {
        let footprint = crate::builders::build_mounting_hole(reference, x, y, drill);
        let item = crate::builders::pack_any(&footprint, "kiapi.board.types.FootprintInstance");
        let commit = self.begin_commit()?;
        match self.create_items(vec![item]) {
            Ok(()) => self.push_commit(&commit, "Add mounting hole"),
            Err(error) => {
                let _ = self.drop_commit(&commit);
                Err(error)
            }
        }
    }

    /// Set the active PCB editor layer.
    pub fn set_active_layer(&self, layer: &str) -> Result<()> {
        let board = self.get_board_document()?;
        let layer = crate::builders::layer_from_name(layer);
        if layer == kiapi::board::types::BoardLayer::BlUndefined {
            anyhow::bail!("Unknown KiCad board layer");
        }
        let cmd = kiapi::board::commands::SetActiveLayer {
            board: Some(board),
            layer: layer as i32,
        };
        self.send_command(&cmd, "kiapi.board.commands.SetActiveLayer")?;
        Ok(())
    }

    /// Serialize the exact live PCB document without saving it to disk.
    pub fn read_live_board(&self) -> Result<String> {
        let doc = self.get_board_document()?;
        let cmd = kiapi::common::commands::SaveDocumentToString {
            document: Some(doc),
        };
        let response = self
            .send_command(&cmd, "kiapi.common.commands.SaveDocumentToString")?
            .ok_or_else(|| anyhow::anyhow!("SaveDocumentToString returned no response"))?;
        let response: kiapi::common::commands::SavedDocumentResponse = unpack_any(&response)?;
        Ok(response.contents)
    }

    /// Create or replace one explicit project netclass through IPC.
    pub fn create_netclass(
        &self,
        name: &str,
        clearance: f64,
        track_width: f64,
        via_drill: f64,
        via_diameter: f64,
    ) -> Result<()> {
        let via_stack =
            crate::builders::build_via("", 0, 0.0, 0.0, via_drill, via_diameter).pad_stack;
        let netclass = kiapi::common::project::NetClass {
            name: name.to_string(),
            priority: None,
            board: Some(kiapi::common::project::NetClassBoardSettings {
                clearance: Some(crate::builders::distance(clearance)),
                track_width: Some(crate::builders::distance(track_width)),
                via_stack,
                ..Default::default()
            }),
            schematic: None,
            r#type: kiapi::common::project::NetClassType::NctExplicit as i32,
            constituents: vec![],
        };
        let cmd = kiapi::common::commands::SetNetClasses {
            net_classes: vec![netclass],
            merge_mode: kiapi::common::types::MapMergeMode::MmmMerge as i32,
        };
        self.send_command(&cmd, "kiapi.common.commands.SetNetClasses")?;
        Ok(())
    }

    /// Create a copper zone polygon through IPC and refill zones in one commit.
    pub fn add_copper_zone(
        &self,
        net_name: &str,
        layer: &str,
        clearance: f64,
        min_width: f64,
        points: &[(f64, f64)],
        holes: &[Vec<(f64, f64)>],
    ) -> Result<()> {
        use kiapi::board::types::{
            zone, CopperZoneSettings, IslandRemovalMode, Zone, ZoneConnectionSettings,
            ZoneConnectionStyle, ZoneFillMode, ZoneType,
        };
        use kiapi::common::types::{
            poly_line_node, PolyLine, PolyLineNode, PolySet, PolygonWithHoles,
        };
        let code = self.resolve_net_code(net_name)?;
        let layer = crate::builders::layer_from_name(layer);
        if layer == kiapi::board::types::BoardLayer::BlUndefined {
            anyhow::bail!("Unknown KiCad board layer");
        }
        let outline = PolySet {
            polygons: vec![PolygonWithHoles {
                outline: Some(PolyLine {
                    nodes: points
                        .iter()
                        .map(|&(x, y)| PolyLineNode {
                            geometry: Some(poly_line_node::Geometry::Point(crate::builders::vec2(
                                x, y,
                            ))),
                        })
                        .collect(),
                    closed: true,
                }),
                holes: holes
                    .iter()
                    .map(|hole| PolyLine {
                        nodes: hole
                            .iter()
                            .map(|&(x, y)| PolyLineNode {
                                geometry: Some(poly_line_node::Geometry::Point(
                                    crate::builders::vec2(x, y),
                                )),
                            })
                            .collect(),
                        closed: true,
                    })
                    .collect(),
            }],
        };
        let zone = Zone {
            id: None,
            r#type: ZoneType::ZtCopper as i32,
            layers: vec![layer as i32],
            outline: Some(outline),
            name: String::new(),
            settings: Some(zone::Settings::CopperSettings(CopperZoneSettings {
                connection: Some(ZoneConnectionSettings {
                    zone_connection: ZoneConnectionStyle::ZcsThermal as i32,
                    thermal_spokes: None,
                }),
                clearance: Some(crate::builders::distance(clearance)),
                min_thickness: Some(crate::builders::distance(min_width)),
                island_mode: IslandRemovalMode::IrmAlways as i32,
                min_island_area: 0,
                fill_mode: ZoneFillMode::ZfmSolid as i32,
                hatch_settings: None,
                net: Some(crate::builders::net(net_name, code)),
                teardrop: None,
            })),
            priority: 0,
            filled: false,
            filled_polygons: vec![],
            border: None,
            locked: kiapi::common::types::LockedState::LsUnlocked as i32,
            layer_properties: vec![],
        };
        let any = crate::builders::pack_any(&zone, "kiapi.board.types.Zone");
        let commit = self.begin_commit()?;
        let result = (|| {
            self.create_items(vec![any])?;
            self.push_commit(&commit, "Add copper zone")
        })();
        if result.is_err() {
            let _ = self.drop_commit(&commit);
            return result;
        }
        self.refill_zones()
    }

    /// Replace the outline of one board-level copper zone while preserving all
    /// electrical and fill settings. Coordinates are board-absolute.
    pub fn update_copper_zone_outline(&self, uuid: &str, points: &[(f64, f64)]) -> Result<()> {
        use kiapi::common::types::{
            poly_line_node, PolyLine, PolyLineNode, PolySet, PolygonWithHoles,
        };

        if points.len() < 3 {
            anyhow::bail!("Copper zone outline requires at least 3 points");
        }
        let items = self.get_items(kiapi::common::types::KiCadObjectType::KotPcbZone)?;
        let mut target = items
            .into_iter()
            .filter_map(|item| kiapi::board::types::Zone::decode(item.value.as_slice()).ok())
            .find(|zone| zone.id.as_ref().is_some_and(|id| id.value == uuid))
            .ok_or_else(|| anyhow::anyhow!("Copper zone '{}' not found", uuid))?;

        target.outline = Some(PolySet {
            polygons: vec![PolygonWithHoles {
                outline: Some(PolyLine {
                    nodes: points
                        .iter()
                        .map(|&(x, y)| PolyLineNode {
                            geometry: Some(poly_line_node::Geometry::Point(crate::builders::vec2(
                                x, y,
                            ))),
                        })
                        .collect(),
                    closed: true,
                }),
                holes: vec![],
            }],
        });
        target.filled = false;
        target.filled_polygons.clear();

        let update = crate::builders::pack_any(&target, "kiapi.board.types.Zone");
        let commit = self.begin_commit()?;
        let result = (|| {
            self.update_items(vec![update])?;
            self.push_commit(&commit, "Update copper zone outline")
        })();
        if result.is_err() {
            let _ = self.drop_commit(&commit);
        }
        result?;
        self.refill_zones()
    }
}

fn translate_polyline(line: &mut kiapi::common::types::PolyLine, dx_nm: i64, dy_nm: i64) {
    use kiapi::common::types::poly_line_node::Geometry;
    for node in &mut line.nodes {
        match &mut node.geometry {
            Some(Geometry::Point(point)) => {
                point.x_nm += dx_nm;
                point.y_nm += dy_nm;
            }
            Some(Geometry::Arc(arc)) => {
                translate_vector(&mut arc.start, dx_nm, dy_nm);
                translate_vector(&mut arc.mid, dx_nm, dy_nm);
                translate_vector(&mut arc.end, dx_nm, dy_nm);
            }
            None => {}
        }
    }
}

fn translate_polyset(polyset: &mut kiapi::common::types::PolySet, dx_nm: i64, dy_nm: i64) {
    for polygon in &mut polyset.polygons {
        if let Some(line) = &mut polygon.outline {
            translate_polyline(line, dx_nm, dy_nm);
        }
        for line in &mut polygon.holes {
            translate_polyline(line, dx_nm, dy_nm);
        }
    }
}

fn translate_zone(zone: &mut kiapi::board::types::Zone, dx_nm: i64, dy_nm: i64) {
    if let Some(outline) = &mut zone.outline {
        translate_polyset(outline, dx_nm, dy_nm);
    }
    for filled in &mut zone.filled_polygons {
        if let Some(shapes) = &mut filled.shapes {
            translate_polyset(shapes, dx_nm, dy_nm);
        }
    }
    for layer in &mut zone.layer_properties {
        translate_vector(&mut layer.hatching_offset, dx_nm, dy_nm);
    }
}

fn translate_graphic_shape(shape: &mut kiapi::common::types::GraphicShape, dx_nm: i64, dy_nm: i64) {
    use kiapi::common::types::graphic_shape::Geometry;
    match &mut shape.geometry {
        Some(Geometry::Segment(segment)) => {
            translate_vector(&mut segment.start, dx_nm, dy_nm);
            translate_vector(&mut segment.end, dx_nm, dy_nm);
        }
        Some(Geometry::Rectangle(rectangle)) => {
            translate_vector(&mut rectangle.top_left, dx_nm, dy_nm);
            translate_vector(&mut rectangle.bottom_right, dx_nm, dy_nm);
        }
        Some(Geometry::Arc(arc)) => {
            translate_vector(&mut arc.start, dx_nm, dy_nm);
            translate_vector(&mut arc.mid, dx_nm, dy_nm);
            translate_vector(&mut arc.end, dx_nm, dy_nm);
        }
        Some(Geometry::Circle(circle)) => {
            translate_vector(&mut circle.center, dx_nm, dy_nm);
            translate_vector(&mut circle.radius_point, dx_nm, dy_nm);
        }
        Some(Geometry::Polygon(polyset)) => {
            for polygon in &mut polyset.polygons {
                if let Some(outline) = &mut polygon.outline {
                    translate_polyline(outline, dx_nm, dy_nm);
                }
                for hole in &mut polygon.holes {
                    translate_polyline(hole, dx_nm, dy_nm);
                }
            }
        }
        Some(Geometry::Bezier(bezier)) => {
            translate_vector(&mut bezier.start, dx_nm, dy_nm);
            translate_vector(&mut bezier.control1, dx_nm, dy_nm);
            translate_vector(&mut bezier.control2, dx_nm, dy_nm);
            translate_vector(&mut bezier.end, dx_nm, dy_nm);
        }
        None => {}
    }
}

fn translate_field(field: &mut Option<kiapi::board::types::Field>, dx_nm: i64, dy_nm: i64) {
    if let Some(text) = field
        .as_mut()
        .and_then(|field| field.text.as_mut())
        .and_then(|text| text.text.as_mut())
    {
        translate_vector(&mut text.position, dx_nm, dy_nm);
    }
}

fn translate_footprint_definition(
    footprint: &mut kiapi::board::types::FootprintInstance,
    dx_nm: i64,
    dy_nm: i64,
) -> Result<()> {
    for field in [
        &mut footprint.reference_field,
        &mut footprint.value_field,
        &mut footprint.datasheet_field,
        &mut footprint.description_field,
    ] {
        translate_field(field, dx_nm, dy_nm);
    }
    let Some(definition) = footprint.definition.as_mut() else {
        return Ok(());
    };
    for field in [
        &mut definition.reference_field,
        &mut definition.value_field,
        &mut definition.datasheet_field,
        &mut definition.description_field,
    ] {
        translate_field(field, dx_nm, dy_nm);
    }
    for item in &mut definition.items {
        if item.type_url.ends_with("kiapi.board.types.Pad") {
            let mut pad = kiapi::board::types::Pad::decode(item.value.as_slice())?;
            translate_vector(&mut pad.position, dx_nm, dy_nm);
            item.value = pad.encode_to_vec();
        } else if item
            .type_url
            .ends_with("kiapi.board.types.BoardGraphicShape")
        {
            let mut graphic =
                kiapi::board::types::BoardGraphicShape::decode(item.value.as_slice())?;
            if let Some(shape) = &mut graphic.shape {
                translate_graphic_shape(shape, dx_nm, dy_nm);
            }
            item.value = graphic.encode_to_vec();
        } else if item.type_url.ends_with("kiapi.board.types.BoardText") {
            let mut board_text = kiapi::board::types::BoardText::decode(item.value.as_slice())?;
            if let Some(text) = &mut board_text.text {
                translate_vector(&mut text.position, dx_nm, dy_nm);
            }
            item.value = board_text.encode_to_vec();
        } else if item.type_url.ends_with("kiapi.board.types.Zone") {
            let mut zone = kiapi::board::types::Zone::decode(item.value.as_slice())?;
            translate_zone(&mut zone, dx_nm, dy_nm);
            item.value = zone.encode_to_vec();
        }
    }
    Ok(())
}
