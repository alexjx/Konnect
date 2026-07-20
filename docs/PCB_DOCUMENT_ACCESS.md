# PCB document access and mutation policy

This document is the contract for PCB access in Konnect. Its first priority is
selecting the correct KiCad document; its second is preventing file APIs from
bypassing KiCad's live editor state and undo model.

The design follows KiCad's IPC architecture: protobuf request/reply over NNG,
synchronous handling on KiCad's UI thread, one active request stream per client,
and an instance token supplied in response envelopes. See the
[KiCad IPC API overview](https://dev-docs.kicad.org/en/apis-and-binding/ipc-api/)
and [developer transport notes](https://dev-docs.kicad.org/en/apis-and-binding/ipc-api/for-kicad-developers/).

## Invariants

1. A path to an existing `.kicad_pcb` grants read access only.
2. PCB mutations go through a `KiCadIpcClient` bound to one exact canonical
   board path. Konnect never chooses the first open board.
3. Explicit `save_project` is an IPC persistence operation on that same bound
   document. It is not a filesystem write.
4. `create_project` is the only direct PCB-write exception. It uses
   create-new/no-overwrite semantics and removes only files created by the
   failed call.
5. Generated reports, renders, and exports may not have a `.kicad_pcb`
   destination or alias the input board.

## Capability model

`ReadOnlyBoardFile::open(path)` canonicalizes an existing regular PCB file and
exposes only `read` and `read_to_string`. It has no write method.

`KiCadIpcClient::bind_board(path)` resolves the path against
`GetOpenDocuments`. A bound client revalidates that exact path before every
document-scoped operation. This client-side project-path check is intentional:
the KiCad 10 wire document specifier includes the project and filename, but PCB
document validation in the editor may be less strict than a full canonical-path
comparison.

`validate_artifact_path(board, output)` protects CLI output boundaries.
`konnect_sexp::write_atomic` independently rejects `.kicad_pcb` destinations as
defence in depth.

## Command scopes

Not every IPC protobuf command contains a document field. Konnect classifies
calls by what they need:

- **Document-scoped:** board queries, item CRUD, refill, save, and live document
  serialization. These require an exact board binding.
- **Document-context safeguard:** `BeginCommit`/`EndCommit` are session-scoped
  in KiCad, but Konnect validates the bound board before beginning a commit so
  a transaction cannot accidentally start without document context.
- **Session-scoped:** `Ping`, `GetVersion`, and `GetOpenDocuments`. These do not
  require a board binding.

This distinction avoids pretending that every KiCad command has a mandatory
document while retaining the safeguard where correctness depends on it.

## Session and failure semantics

All tool calls share one logical IPC session. Clones share:

- the KiCad instance token;
- a request gate that serializes synchronous NNG calls;
- a unique client name.

A fresh NNG request socket is created for each command so a timed-out REQ socket
is never reused. The first valid response pins the instance token; a later
mismatch fails the call. API status errors remain distinct from malformed
responses. If receive fails after a request was sent, the error is
`OutcomeUnknown`; mutating commands are not automatically retried because KiCad
may already have applied them.

CRUD responses are checked at both request and per-item levels, including
response cardinality. Commit creation must return a non-empty ID.

## Public operations

- `read_pcb_document(board)` returns the exact live editor document through
  `SaveDocumentToString` without saving it.
- Existing PCB analysis tools may read the on-disk board through the read-only
  capability when live editor state is not required.
- `save_project(board)` explicitly saves the exact bound live board through
  IPC.
- All PCB editing toolsets bind their `board` argument before issuing IPC
  queries or mutations.

## KiCad 10 compatibility baseline

The vendored protobufs include the KiCad 10 maintenance additions used as the
current baseline: net-based item filtering, connected-item query, barcode and
reference-image types, and title-block mutation. Runtime `AS_UNHANDLED` and
`AS_UNIMPLEMENTED` statuses still take precedence over assumptions based only
on version numbers.

When updating KiCad support, compare the vendored files with the matching
release under KiCad's
[official KiCad 10.0.4 protobuf tree](https://gitlab.com/kicad/code/kicad/-/tree/10.0.4/api/proto),
then update conformance tests before exposing new commands.
