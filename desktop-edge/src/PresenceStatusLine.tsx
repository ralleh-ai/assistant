import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * Screen-reader-oriented status line for the presence entity
 * (Phase 4 accessibility). Polls the shell's mode tracker at ~5 Hz
 * and renders a present-tense phrase — "Listening", "Thinking",
 * etc. — inside an `aria-live="polite"` region so assistive tech
 * announces state changes to the operator without the operator
 * having to look at the visual.
 *
 * # Why polling instead of events
 *
 * The Rust `Presence` tracker is the single source of truth for
 * "what modes are on air right now". Emitting a Tauri event on
 * every mode change would give sub-poll latency, but the wire cost
 * is meaningful in the middle of a mic pump (~30 Hz mode-adjacent
 * commands) and the accessibility gain from sub-200 ms latency is
 * zero — screen readers coalesce announcements anyway. A cheap 5
 * Hz poll gives the same experience with a much simpler failure
 * model: no listener lifecycle, no missed events on remount.
 *
 * # Priority order
 *
 * The runtime can be in several modes at once (Speaking layered on
 * Attention, for instance). This component picks the single most
 * salient one for the announcement rather than reading a list —
 * one visible phrase, sorted by "what the operator needs to know
 * first". Error trumps everything; Speaking beats Attention beats
 * Listening beats Thinking beats ToolUse. Idle is the silence
 * that remains when nothing is engaged.
 */
export function PresenceStatusLine() {
    const [phrase, setPhrase] = useState<string>("Idle");

    useEffect(() => {
        let cancelled = false;
        // M10: skip a tick while the previous IPC call is still
        // outstanding. A fixed 200 ms `setInterval` firing an async
        // callback stacks calls if the shell is momentarily slow
        // (mid mic-pump, GC pause), which both wastes IPC and can
        // apply an older snapshot after a newer one. `inFlight`
        // collapses those to at most one outstanding read.
        let inFlight = false;
        // 5 Hz. Tracker reads are `HashSet::iter().collect()` under
        // a `Mutex` — sub-microsecond critical sections — so the
        // cost of this timer is dominated by IPC overhead, not by
        // contention. Interval chosen to feel responsive without
        // spamming the log at info level if the poll ever errors.
        const tick = async () => {
            if (inFlight) return;
            inFlight = true;
            try {
                const modes = await invoke<string[]>("presence_current_modes");
                if (!cancelled) setPhrase(pickPhrase(modes));
            } catch {
                // Presence disabled or shell shutting down. Retain
                // the last known phrase rather than flashing "Idle";
                // the screen reader has already announced whatever
                // was on air last.
            } finally {
                inFlight = false;
            }
        };
        void tick();
        const id = window.setInterval(() => void tick(), 200);
        return () => {
            cancelled = true;
            window.clearInterval(id);
        };
    }, []);

    return (
        <p
            className="presence-status-line"
            role="status"
            aria-live="polite"
            aria-atomic="true"
        >
            {phrase}
        </p>
    );
}

// Mode labels come from `PresenceMode::label()` on the Rust side.
// Priority order documented in the component header.
const PRIORITY: readonly string[] = [
    "error",
    "speaking",
    "attention",
    "listening",
    "thinking",
    "tool_use",
];

const PHRASES: Record<string, string> = {
    error: "Error",
    speaking: "Speaking",
    attention: "Something to see",
    listening: "Listening",
    thinking: "Thinking",
    tool_use: "Using a tool",
};

export function pickPhrase(modes: string[]): string {
    for (const key of PRIORITY) {
        if (modes.includes(key)) return PHRASES[key];
    }
    return "Idle";
}
