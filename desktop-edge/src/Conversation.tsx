import { useEffect, useRef, useState } from "react";
import { assistantThinkStream } from "./presence";

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
    // `pending` is true from send until the terminal event. `partial`
    // is the streaming assistant text — non-null while chunks are
    // arriving, folded into a real message on the terminal event.
    // Splitting them lets the UI show the three-dot indicator until
    // the first chunk arrives, then swap to a real growing bubble
    // — matches every well-behaved streaming chat UI.
    const [pending, setPending] = useState(false);
    const [partial, setPartial] = useState<string | null>(null);
    const nextId = useRef(1);
    const scrollRef = useRef<HTMLDivElement | null>(null);
    // M11: the streaming channel keeps firing its callback until the
    // terminal event even if the operator navigates away from Core
    // mid-stream (unmounting this component). Without a guard, those
    // late callbacks call `setState` on an unmounted tree — a React
    // warning today and a latent bug once real backends stream for
    // many seconds. `mounted` is flipped false on unmount and checked
    // before every state write below. (A future improvement is to pass
    // an abort signal through to Rust so the stream is torn down
    // server-side too, not just ignored here.)
    const mounted = useRef(true);
    useEffect(
        () => () => {
            mounted.current = false;
        },
        [],
    );

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
    }, [messages, pending, partial]);

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
        setPartial(null);

        // Accumulate chunks locally rather than in React state so
        // the terminal handler has a synchronous, atomic view of
        // the full text — setState in the handler is asynchronous
        // and would race with the terminal event.
        let buffer = "";
        const commitAsMessage = (role: Role, text: string) => {
            if (!mounted.current) return;
            setMessages((prev) => [
                ...prev,
                { id: nextId.current++, role, text },
            ]);
        };

        try {
            await assistantThinkStream(prompt, (event) => {
                if (!mounted.current) return;
                switch (event.event) {
                    case "chunk":
                        buffer += event.text;
                        setPartial(buffer);
                        break;
                    case "done":
                        commitAsMessage("assistant", buffer);
                        break;
                    case "failed":
                        commitAsMessage(
                            "error",
                            `completion failed via ${event.backend}: ${event.error}`,
                        );
                        break;
                    case "denied":
                        commitAsMessage(
                            "error",
                            "policy denied the completion request",
                        );
                        break;
                    case "approval_required":
                        commitAsMessage(
                            "error",
                            "completion requires human approval",
                        );
                        break;
                    case "no_backend_configured":
                        commitAsMessage(
                            "error",
                            "no completion backend is configured on this shell",
                        );
                        break;
                }
            });
        } catch (err) {
            commitAsMessage("error", String(err));
        } finally {
            if (mounted.current) {
                setPending(false);
                setPartial(null);
            }
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
                {partial !== null && partial.length > 0 && (
                    <div
                        className="conversation-bubble is-assistant is-streaming"
                        aria-label="Assistant is speaking"
                    >
                        {partial}
                    </div>
                )}
                {pending && (partial === null || partial.length === 0) && (
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
