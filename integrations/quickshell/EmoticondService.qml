pragma Singleton
pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import Quickshell.Io

/**
 * EmoticondService: a Quickshell client for `emoticond serve` (protocol v1,
 * docs/protocol.md). It owns the daemon process and the protocol state; it
 * draws nothing. Put it in one of your shell's module directories (it is a
 * singleton: the file name is the type name) and bind your picker to it.
 *
 * Lifecycle
 *   - Nothing runs until you call ensureRunning() (do it when your launcher
 *     or picker opens, so the engine is up by the first keystroke) or
 *     search(). Start-up takes tens of ms.
 *   - The daemon exits by itself after `idleSeconds` without a request.
 *     That exit (status 0) is not respawned; the next ensureRunning() or
 *     search() starts it again, and `ready` flipping back to true is your cue
 *     to re-send the current query.
 *   - A crash after a successful start is retried once. A daemon that never
 *     comes up (missing binary, no data, wrong protocol) sets `failed`
 *     (also after `startTimeoutMs`) so you can fall back to something else.
 *
 * Searching
 *   - search(text) on every keystroke. Each request goes on one supersession
 *     channel, so stale queued ones are answered "superseded" by the daemon,
 *     and only the newest answer is applied. "" gives the browse list
 *     (`mode` "browse": canonical starter faces, then your history; each
 *     result has `from`).
 *   - `results` is kaomoji only: { id, text, kind, score, rank, from? }.
 *     Use `id` as the row key. Emoji and symbols are not in the library;
 *     mix in your own list if you want them.
 *   - `reading` is the one-line reading for under the search box,
 *     `corrected` the word a misspelled query was read as
 *     ("Showing results for X"; searchUncorrected() re-runs without it).
 *
 * Feedback (the two report menus)
 *   - Query menu: reportQuery("read_well" | "read_wrong" | "missing"),
 *     noteQuery(text). Face menu: reportFace(id, rank, "great_fit" |
 *     "other_word" | "no_fit" | "offensive"), noteFace(id, rank, text).
 *     Choosing the chosen item again withdraws it (`clear`).
 *   - "offensive" removes the row from `results` at once (the daemon has
 *     blocklisted it everywhere) and sets `hiddenFace`; undoHide() unblocks
 *     it and puts it back ("Hidden. Undo").
 *   - Show `reportFooter` (the shipped disclaimer line, or the last ack's
 *     footer) inside both menus; never write your own wording.
 *     `disclaimerText` is the full text.
 *   - pick(id, rank) when the user copies a face (local popularity).
 */
Singleton {
    id: root

    // ---- Configuration ------------------------------------------------------
    // The emoticond binary: on PATH by default, or a full path.
    property string binary: "emoticond"
    // A development checkout to read data from (EMOTICOND_REPO); empty uses
    // the normal data search (~/.local/share/emoticond/ and so on).
    property string repo: ""
    // Selects [profile.<frontend>] in the user's config.toml.
    property string frontend: "quickshell"
    property int idleSeconds: 120
    // Sent once per daemon start with set_defaults.
    property int resultLimit: 100
    // Extra options for set_defaults, e.g. ({ safety: "moderate" }).
    property var defaults: ({})
    property int startTimeoutMs: 5000

    // ---- Daemon state -------------------------------------------------------
    property bool ready: false
    property bool failed: false
    property string lastError: ""
    readonly property bool running: proc.running
    // From the ready line.
    property var counts: ({})
    property var features: []
    property string disclaimerShort: ""
    property string disclaimerText: ""
    property int disclaimerVersion: 0
    // False when the user or the packager turned report sending off.
    property bool sending: true
    property string popularity: ""

    // ---- The shown search ---------------------------------------------------
    property var results: []
    property string query: ""
    property string mode: ""
    // The request id `results` came from: the `about` of picks and reports.
    property var searchId: null
    property string corrected: ""
    property string reading: ""
    // concept, terms, situation, sentence, partial, glyph, tags, nothing, empty
    property string readingKind: ""
    // For a partial query ("completing: sad, salty"): the completions.
    property var completions: []

    // ---- Report menus -------------------------------------------------------
    property string footer: ""
    readonly property string reportFooter: root.footer.length > 0 ? root.footer
        : !root.sending ? "Reports are saved on this computer only."
        : root.disclaimerShort
    // "read_well" / "read_wrong" / ""; exclusive.
    property string readVerdict: ""
    property bool missing: false
    property bool queryNoted: false
    // { face id: reason }
    property var faceReports: ({})
    // { id, text, rank, index } of the last face reported offensive.
    property var hiddenFace: null

    // New `results` were applied.
    signal searched()
    // A report was answered: { ok, queued, sent, sending, footer, effects, ... }.
    signal reportAcked(var ack)

    /** Start the daemon if it is not running. Returns immediately. */
    function ensureRunning(): void {
        if (proc.running)
            return;
        proc.running = true;
        startWatchdog.restart();
    }

    /** Ask the daemon to exit now (it would idle out anyway). */
    function stop(): void {
        if (proc.running)
            root._send({ op: "quit" });
    }

    property int _nextId: 0
    property int _latestSearch: -1

    function _send(msg: var): void {
        proc.write(JSON.stringify(msg) + "\n");
    }

    /** One call per keystroke. `opts` are per-request search options. */
    function search(text: string, opts: var): void {
        if (!root.ready) {
            if (!root.failed)
                root.ensureRunning();
            return;
        }
        root._nextId += 1;
        root._latestSearch = root._nextId;
        root._send({
            op: "search",
            id: root._nextId,
            q: text,
            chan: "main",
            opts: Object.assign({ explain: "reading" }, opts ?? {})
        });
    }

    /** "Search instead for <typed>": the shown query without spelling correction. */
    function searchUncorrected(): void {
        root.search(root.query, { correct: false });
    }

    /** The user copied face `face` (an id) shown at `rank`. */
    function pick(face: string, rank: int): void {
        if (!root.ready || !face)
            return;
        root._nextId += 1;
        root._send({ op: "pick", id: `p${root._nextId}`, about: root.searchId, q: root.query, face: face, rank: rank });
    }

    // `about` names the search; `q` lets the daemon rebuild the context if it
    // restarted since.
    function _report(fields: var): void {
        if (!root.ready)
            return;
        root._nextId += 1;
        root._send(Object.assign({ op: "report", id: `r${root._nextId}`, about: root.searchId, q: root.query }, fields));
    }

    /**
     * Query menu item. The daemon keeps one report per query (a later one
     * supersedes an earlier one), so withdrawing one choice while the other
     * is still chosen re-sends that one; withdrawing the last sends `clear`.
     */
    function reportQuery(reason: string): void {
        let on;
        if (reason === "missing") {
            root.missing = !root.missing;
            on = root.missing;
        } else {
            root.readVerdict = root.readVerdict === reason ? "" : reason;
            on = root.readVerdict === reason;
        }
        if (on)
            root._report({ reason: reason });
        else if (root.readVerdict || root.missing)
            root._report({ reason: root.readVerdict || "missing" });
        else
            root._report({ reason: "clear" });
    }

    /** Query menu "Custom…": goes with the chosen reason, or as `note`. */
    function noteQuery(text: string): void {
        const note = text.trim();
        if (!note)
            return;
        root._report({ reason: root.readVerdict || (root.missing ? "missing" : "note"), note: note });
        root.queryNoted = true;
    }

    /** Face menu item; the same item again sends `clear`. */
    function reportFace(face: string, rank: int, reason: string): void {
        if (!face)
            return;
        const next = Object.assign({}, root.faceReports);
        if (next[face] === reason) {
            delete next[face];
            root.faceReports = next;
            root._report({ reason: "clear", face: face, rank: rank });
            return;
        }
        next[face] = reason;
        root.faceReports = next;
        root._report({ reason: reason, face: face, rank: rank });
        if (reason === "offensive") {
            const index = root.results.findIndex(r => r.id === face);
            if (index >= 0) {
                root.hiddenFace = { id: face, text: root.results[index].text, rank: rank, index: index };
                root.results = root.results.filter(r => r.id !== face);
            }
        }
    }

    /** Face menu "Custom…": goes with the face's chosen reason, or as `note`. */
    function noteFace(face: string, rank: int, text: string): void {
        const note = text.trim();
        if (!face || !note)
            return;
        const reason = root.faceReports[face] ?? "";
        root._report({ reason: reason || "note", face: face, rank: rank, note: note });
        if (!reason) {
            const next = Object.assign({}, root.faceReports);
            next[face] = "note";
            root.faceReports = next;
        }
    }

    /** Undo the last offensive report: unblock the face and put its row back. */
    function undoHide(): void {
        const h = root.hiddenFace;
        if (!h)
            return;
        root._report({ reason: "clear", face: h.id, rank: h.rank });
        const next = Object.assign({}, root.faceReports);
        delete next[h.id];
        root.faceReports = next;
        const rows = root.results.slice();
        rows.splice(Math.min(h.index, rows.length), 0, { id: h.id, text: h.text, kind: "emoticon", rank: h.rank });
        root.results = rows;
        root.hiddenFace = null;
    }

    function _handle(line: string): void {
        let msg;
        try {
            msg = JSON.parse(line);
        } catch (e) {
            root.lastError = `unparseable line: ${line.slice(0, 80)}`;
            return;
        }
        if (msg.ready) {
            if ((msg.protocol ?? 0) < 1) {
                root.lastError = "the daemon does not speak protocol v1";
                root.failed = true;
                startWatchdog.stop();
                root._send({ op: "quit" });
                return;
            }
            root.counts = msg.counts ?? ({});
            root.features = msg.features ?? [];
            root.disclaimerShort = msg.disclaimer?.short ?? "";
            root.disclaimerText = msg.disclaimer?.text ?? "";
            root.disclaimerVersion = msg.disclaimer?.version ?? 0;
            root.sending = msg.sending ?? true;
            root.popularity = msg.popularity ?? "";
            root.failed = false;
            startWatchdog.stop();
            root._send({
                op: "set_defaults",
                id: "defaults",
                opts: Object.assign({ limit: root.resultLimit, explain: "reading" }, root.defaults)
            });
            root.ready = true;
            return;
        }
        if (msg.ok === false) {
            if (msg.superseded || msg.cancelled)
                return;
            root.lastError = msg.error?.message ?? `request ${msg.id} failed`;
            console.warn(`[EmoticondService] ${msg.error?.code ?? "error"}: ${root.lastError}`);
            return;
        }
        if (typeof msg.id === "string") {
            if (msg.id.startsWith("r")) {
                if (msg.footer)
                    root.footer = msg.footer;
                if (msg.sending !== undefined)
                    root.sending = msg.sending;
                root.reportAcked(msg);
            }
            return;
        }
        if (msg.id !== root._latestSearch)
            return;
        const q = msg.q ?? "";
        const sameSearch = q === root.query && (msg.mode ?? "") === root.mode;
        root.searchId = msg.id;
        root.query = q;
        root.mode = msg.mode ?? "";
        root.corrected = msg.corrected ?? "";
        root.reading = msg.reading?.line ?? "";
        root.readingKind = msg.reading?.mode?.kind ?? "";
        root.completions = msg.reading?.mode?.completions ?? [];
        if (!sameSearch) {
            root.readVerdict = "";
            root.missing = false;
            root.queryNoted = false;
            root.faceReports = ({});
            root.hiddenFace = null;
        }
        root.results = msg.results ?? [];
        root.searched();
    }

    Process {
        id: proc
        command: [root.binary, "serve", "--frontend", root.frontend, "--idle", `${root.idleSeconds}`]
        // Merged into the inherited environment, so EMOTICOND_PICK_LOG,
        // EMOTICOND_DATA_FILE etc. still pass through.
        environment: root.repo.length > 0 ? ({ "EMOTICOND_REPO": root.repo }) : ({})
        running: false
        stdinEnabled: true

        stdout: SplitParser {
            onRead: line => root._handle(line)
        }
        stderr: SplitParser {
            onRead: line => {
                root.lastError = line;
            }
        }

        onExited: (exitCode, exitStatus) => {
            const wasReady = root.ready;
            root.ready = false;
            startWatchdog.stop();
            // 0: idle exit or quit. Started again on demand.
            if (exitCode === 0)
                return;
            root.lastError = `emoticond serve exited with ${exitCode}`;
            if (wasReady)
                root.ensureRunning();
            else
                root.failed = true;
        }
    }

    // A binary that cannot be spawned never exits, because it never ran.
    Timer {
        id: startWatchdog
        interval: root.startTimeoutMs
        repeat: false
        onTriggered: {
            if (!root.ready)
                root.failed = true;
        }
    }
}
