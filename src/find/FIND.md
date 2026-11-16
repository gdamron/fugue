# Finding what Fugue offers

An agent asks in a few key words ("reverb", "make it brighter", "edit a
note", "render a preview", "what can I pin") and gets back a short ranked
list of cards, each saying what a thing is, when to use it and the exact
call or document fragment that uses it. One search covers a host's tools
and views, the module types, developments, examples and packages the daemon
knows, and the loaded piece's own controls. A new module, view or package
adds a card to the index; it adds nothing to a session's tool listing.

`fugue::find` holds the index, the ranking and the synonym table. A host
builds an index per search: `Gather` reads the daemon's side through the
discovery reads the daemon already answers, and the host adds the cards for
its own tools and views. Fyug's MCP server offers the search as its `find`
tool, and `fyug find` offers it on the command line.

## Query

```json
{"words": "make it brighter", "kind": "control", "limit": 5}
```

- `words` (required): key words or intent, 1 to 256 bytes.
- `kind` (optional): only cards of this kind: `tool`, `view`, `module_type`,
  `development`, `example`, `package` or `control`.
- `limit` (optional): the most cards, 1 to 20; 5 when left out.

An empty or oversized `words`, an unknown kind or a limit out of range is
refused as an invalid request. A query that matches nothing is an answer.

## Answer

```json
{
  "schema_version": 1,
  "cards": [{"kind": "control", "name": "lead.cutoff", "what": "...", "when": "...", "how": {}}],
  "more": 3,
  "notes": ["3 more matched; raise limit (up to 20) or add words or a kind."]
}
```

- `cards`: best first. Each has `kind`, `name` (the exact name to use it
  by), `what` (what it is), `when` (when to use it) and `how` (below).
- `more`: how many more cards matched than are listed; left out when none.
- `notes`: what the reader should know. A list cut short says how many
  more matched and why: the limit, or the answer's byte cap of 24,576 bytes
  (`MAX_FIND_BYTES`). Cards are never cut part way. With no piece loaded, a
  search of every kind or of controls says so. A source the daemon could
  not read is named, and so is a catalog page's count of entries it left
  out as unreadable; the search answers from the rest. An index keeps at
  most eight notes of at most 240 characters each and counts the rest, so
  every answer fits the byte cap. A host that
  leaves a kind out of its index (`Index::leave_out`) says so, and why,
  when a query asks for that kind or matches nothing: `fyug find` holds no
  tools or views, which only the MCP `find` tool searches.
- `nearest`: only when nothing matched. `terms` are up to five words the
  index holds that are spelled closest to the query's (when none is spelled
  close, the closest three); `kinds` are the kinds the query matches
  without its `kind` filter (an empty list when it matches none). A host
  leaves out each kind for one reason, cut like a note.

The same index and query always give the same answer.

## What a card holds, by kind

| kind | `name` | `how` |
|---|---|---|
| `tool` | the tool's name | `call`: one worked call, `{"tool", "arguments"}`, its required arguments (and the first of a set to pass exactly one of) holding a value the tool's definition fixes, allows or states, or a `<name>` placeholder to replace; `arguments`: the tool's full input schema, as the server defines it (not a thinned listing) |
| `view` | the view, as `show_view` names it | `call`: the `show_view` call that opens it, with each argument the view needs (placeholders as for a tool); `arguments`: what each argument it reads means |
| `module_type` | the registered type | `module`: a document fragment `{"id", "type", "config"}` whose config sets up to six of its controls to their defaults; `inputs` and `outputs`: its ports; `detail`: the `describe_module` call for the rest |
| `development` | the content id | `declare`: the `developments` entry `{"name", "ref"}` that brings it into a piece; `module`: a module using it; `detail`: the `describe_development` call |
| `example` | the content id | `call`: the `load_invention` call that plays it; `detail`: the `describe_example` call |
| `package` | the package id | `call`: the `list_packages` call for its detail; `ref`: its content reference, for a development or invention package |
| `control` | `module_id.key` | `call`: the `set_control` call that reaches it, with its current value, or `get_control` for a control only its module writes or whose value is too long to repeat (over 64 characters) |

A control card's `what` names the control, its module (as the piece names
it) and the module's type; its `when` holds the control's description, its
current value and its range or choices. A module type's `when` names its
controls. A package of a kind this build does not know is listed like any
other. Searching every kind, a package listed after the development or
example it holds is left out, as the same thing twice; asking for packages
lists it.

## Ranking

Matching is lexical. Words split at anything but a letter or digit and at
a lowercase letter followed by a capital, are lowercased and made singular
by their ending ("notes" is "note"), and common words that carry no meaning
for a search ("make", "it", "the", "add") are dropped from the query, unless
it holds nothing else. Each word of the
query is matched against each card's words, weighted by where the card
holds it: its name most, then `what`, then its key words (a module type's
control keys, labels, choices and ports; a control's module and choices),
then `when` and longer prose (a module type's control descriptions). A word that starts another (three letters or more)
matches it at a lower weight, and so does a word the synonym table widens
the query word to. Rare words count more than common ones. A card that
matches more of the query's words ranks ahead of one that matches fewer,
whatever their weights, and a query
that names a card outright ranks it far ahead. Ties fall by kind (control,
module type, tool, view, development, example, package), then by name.

## Synonyms

`fugue::find::synonyms::SYNONYMS` is the one table of musical synonyms:
each row pairs words a musician might use ("brighter", "echo", "faster")
with Fugue's words for them ("cutoff", "delay", "bpm"). Add a row, or a word
to one, when a musical request misses what it means, and keep Fugue's words
to ones the index holds. There is no learned model and no network access.

## What the daemon is asked

`Gather` sends, in order: the snapshot (the piece's controls, and whether
one is loaded); the module type names, then their full descriptions 32 at a
time (a batch the daemon refuses, say for its response cap, is asked again
in halves, and a type refused alone keeps a card by name); every page of
the example and development catalogs and the package list, 100 entries a
page and at most five pages each. Each read is one the
daemon already answers for `describe_module_types`, `list_examples`,
`list_developments`, `list_packages` and `list_controls`; none changes
anything. A host searches from its own process; nothing runs on the audio
thread.
