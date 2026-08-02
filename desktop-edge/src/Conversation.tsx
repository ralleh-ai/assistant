import { useEffect, useRef, useState } from "react";
import { assistantThink } from "./presence";

/**
 * Minimal conversation surface on top of `assistant_think`.
 *
 * Product-side counterpart to Phase 3 §3.2: the dev panel's "Think"
 * chip has proven the router → presence path end-to-end, and this
 * is the first UX that actually lets an operator drive that path.
 * The completion backend is still `EchoBackend` today — the reply
 * is `"echo: <prompt>"` — but the surface is intentionally
 * indistinguishable from what a real completion will look like:
 * a message bubble that arrives after the presence's `thinking`
 * mode has been engaged for the duration of the call. When the
 * router swaps to a real backend, this component does not change.
 *
 * # Scope
 *
 * Deliberately kept small for the first landing:
 * - Local-only message history (not persisted, not shared across
 *   sessions). Persistence is a settings/config question we
 *   haven't answered yet.
 * - No streaming. `assistantThink` today is call-and-reply; when
 *   a streaming variant lands, this component grows a
 *   partial-message state next to `pending`.
 * - No retries. A failed completion surfaces the error inline and
 *   leaves the user's message in place so they can try again by
 *   pressing send again on the same input, or edit and resend.
 * - Enter sends, Shift+Enter newlines. Standard chat behavior.
 */

type Role = "user" | "assistant" | "error";

type Message = {
    id: number;
    role: Role;
    text: string;
};

export function Conversation() {
    const [messages, setMessages] = useState<Message[]>([]);
    const [draft, setDraft] = useState("");
    const [pending, setPending] = useState(false);
    const nextId = useRef(1);
    const scrollRef = useRef<HTMLDivElement | null>(null);

    // Keep the newest message visible without hijacking scroll if
    // the user has scrolled up mid-conversation. `scrollIntoView`
    // with `nearest` respects the user's position when they are
    // actively reading history above; only pins to bottom when
    // already near it. Matches every well-behaved chat UI.
    useEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        const nearBottom =
            el.scrollHeight - el.scrollTop - el.clientHeight < 120;
        if (nearBottom) {
            el.scrollTop = el.scrollHeight;
        }
    }, [messages, pending]);

    async function send() {
        const prompt = draft.trim();
        if (!prompt || pending) return;
        const userMsg: Message = {
            id: nextId.current++,
            role: "user",
            text: prompt,
        };
        setMessages((prev) => [...prev, userMsg]);
        setDraft("");
        setPending(true);
        try {
            const reply = await assistantThink(prompt);
            setMessages((prev) => [
                ...prev,
                { id: nextId.current++, role: "assistant", text: reply },
            ]);
        } catch (err) {
            setMessages((prev) => [
                ...prev,
                { id: nextId.current++, role: "error", text: String(err) },
            ]);
        } finally {
            setPending(false);
        }
    }

    function onKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
        if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            void send();
        }
    }

    return (
        <section
            className="conversation"
            aria-label="Conversation with the assistant"
        >
            <div
                ref={scrollRef}
                className="conversation-scroll"
                role="log"
                aria-live="polite"
                aria-relevant="additions"
            >
                {messages.length === 0 && !pending && (
                    <p className="conversation-empty">
                        Start a conversation. Enter to send, Shift+Enter for a new line.
                    </p>
                )}
                {messages.map((m) => (
                    <div
                        key={m.id}
                        className={`conversation-bubble is-${m.role}`}
                    >
                        {m.text}
                    </div>
                ))}
                {pending && (
                    <div
                        className="conversation-bubble is-pending"
                        aria-label="Assistant is thinking"
                    >
                        <span className="conversation-dot" />
                        <span className="conversation-dot" />
                        <span className="conversation-dot" />
                    </div>
                )}
            </div>

            <form
                className="conversation-composer"
                onSubmit={(e) => {
                    e.preventDefault();
                    void send();
                }}
            >
                <textarea
                    className="conversation-input"
                    placeholder="Message the assistant"
                    value={draft}
                    onChange={(e) => setDraft(e.target.value)}
                    onKeyDown={onKeyDown}
                    rows={2}
                    disabled={pending}
                    aria-label="Message"
                />
                <button
                    type="submit"
                    className="conversation-send"
                    disabled={pending || draft.trim().length === 0}
                >
                    Send
                </button>
            </form>
        </section>
    );
}
