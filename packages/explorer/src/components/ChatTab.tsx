import { useState, useEffect, useRef, useCallback } from "react";
import { useWebSocketContext } from "../context/WebSocketContext";
import {
  loadSessions,
  getSession,
  deleteSession,
  initiateHandshake,
  acceptHandshake,
  completeHandshake,
  encryptMessage,
  decryptMessage,
  parseEnvelope,
  serializeEnvelope,
  isHandshakeEnvelope,
  isDMEnvelope,
  getForwardSecrecyInfo,
  getCachedPlaintext,
  setCachedPlaintext,
  setCurrentWallet,
  getDismissedAt,
  clearDismissed,
  type ChatSession,
  type EncryptedEnvelope,
  type HandshakeEnvelope,
} from "../ratchet";
import { useRinku } from "../context/WalletContext";
import { API_URL } from "../config";

interface ChatTabProps {
  onWalletOpen: () => void;
}

interface ChatMessage {
  hash: string;
  from: string;
  to: string;
  timestamp: number;
  plaintext?: string;
  encrypted: boolean;
  decryptError?: string;
  status?: string;
  direction: "sent" | "received";
}

export function ChatTab({ onWalletOpen }: ChatTabProps) {
  const { wallet, submitTransaction } = useRinku();
  const [sessions, setSessions] = useState<Record<string, ChatSession>>({});
  const [activePeer, setActivePeer] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [newMessage, setNewMessage] = useState("");
  const [newPeerAddress, setNewPeerAddress] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [pendingInvites, setPendingInvites] = useState<
    { from: string; ecdhPub: string; hash: string; txTs?: number }[]
  >([]);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const decryptedHashesRef = useRef<Map<string, string>>(new Map());
  const sentPlaintextsRef = useRef<Map<string, string>>(new Map());

  const myAddress = wallet?.fingerprint || "";

  useEffect(() => {
    setCurrentWallet(myAddress || null);
    if (myAddress) {
      setSessions(loadSessions());
    } else {
      setSessions({});
    }
  }, [myAddress]);

  const submitTx = async (memo: string, to: string) => {
    if (!wallet) throw new Error("Wallet not connected");
    return submitTransaction({
      to,
      amount: 0,
      kind: "transfer",
      gasPrice: 0.001,
      memo,
    });
  };

  const startChat = async () => {
    if (!wallet) {
      onWalletOpen();
      return;
    }
    const peer = newPeerAddress.trim();
    if (!peer || peer.length < 10) {
      setError("Enter a valid wallet address");
      return;
    }
    if (peer === myAddress) {
      setError("Cannot chat with yourself");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      clearDismissed(peer);
      const { envelope } = await initiateHandshake(myAddress, peer);
      try {
        await submitTx(serializeEnvelope(envelope), peer);
      } catch (txErr: any) {
        deleteSession(peer);
        setSessions(loadSessions());
        const msg = txErr.message || "";
        if (msg.includes("Account does not exist")) {
          throw new Error(
            "Your wallet has no on-chain account yet. Use the faucet to get tokens first.",
          );
        }
        throw txErr;
      }
      setSessions(loadSessions());
      setActivePeer(peer);
      setNewPeerAddress("");
      setSuccess("Key exchange request sent! Waiting for peer to accept...");
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  const handleAcceptInvite = async (invite: {
    from: string;
    ecdhPub: string;
  }) => {
    if (!wallet) {
      onWalletOpen();
      return;
    }
    setLoading(true);
    setError(null);
    try {
      clearDismissed(invite.from);
      const { envelope } = await acceptHandshake(
        myAddress,
        invite.from,
        invite.ecdhPub,
      );
      try {
        await submitTx(serializeEnvelope(envelope), invite.from);
      } catch (txErr: any) {
        deleteSession(invite.from);
        setSessions(loadSessions());
        const msg = txErr.message || "";
        if (msg.includes("Account does not exist")) {
          throw new Error(
            "Your wallet has no on-chain account yet. Use the faucet to get tokens first.",
          );
        }
        throw txErr;
      }
      setSessions(loadSessions());
      setActivePeer(invite.from);
      setPendingInvites((prev) => prev.filter((i) => i.from !== invite.from));
      setSuccess("Key exchange complete! You can now send encrypted messages.");
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  const sendMessage = async () => {
    if (!wallet || !activePeer) return;
    const text = newMessage.trim();
    if (!text) return;
    setLoading(true);
    setError(null);
    try {
      const envelope = await encryptMessage(activePeer, text);
      const result = await submitTx(serializeEnvelope(envelope), activePeer);
      const txHash = result?.hash || Date.now().toString();
      sentPlaintextsRef.current.set(txHash, text);
      setCachedPlaintext(activePeer, txHash, text);
      setMessages((prev) => [
        ...prev,
        {
          hash: txHash,
          from: myAddress,
          to: activePeer,
          timestamp: Date.now(),
          plaintext: text,
          encrypted: true,
          direction: "sent",
          status: "pending",
        },
      ]);
      setNewMessage("");
      setSessions(loadSessions());
    } catch (e: any) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  };

  const scanForMessages = useCallback(async () => {
    if (!wallet || !myAddress) return;
    try {
      const res = await fetch(
        `${API_URL}/account/${myAddress}/transactions?limit=100`,
      );
      if (!res.ok) return;
      const data = await res.json();
      const txs: any[] = data.transactions || data || [];

      const invites: {
        from: string;
        ecdhPub: string;
        hash: string;
        txTs: number;
      }[] = [];
      const currentSessions = loadSessions();

      for (const tx of txs) {
        const memo = tx.memo || tx.transaction?.memo;
        const from = tx.from || tx.transaction?.from;
        const to = tx.to || tx.transaction?.to;
        if (!memo || !from) continue;

        const env = parseEnvelope(memo);
        if (!env) continue;

        if (isHandshakeEnvelope(env)) {
          const hs = env as HandshakeEnvelope;
          if (hs.type === "dm_init" && to === myAddress && from !== myAddress) {
            const existingSession = currentSessions[from];
            if (existingSession && existingSession.status === "active") {
            } else if (
              existingSession &&
              existingSession.status === "pending_sent"
            ) {
              const txTs =
                tx.timestamp || tx.transaction?.timestamp || tx.ts || 0;
              if (txTs >= (existingSession.createdAt || 0)) {
                try {
                  await completeHandshake(from, hs.ecdhPub);
                  const updated = loadSessions();
                  setSessions(updated);
                  currentSessions[from] = updated[from];
                } catch (e) {
                  console.error("Failed to auto-resolve cross-initiation:", e);
                }
              }
            } else if (
              !existingSession ||
              existingSession.status === "pending_received"
            ) {
              const txTs =
                tx.timestamp || tx.transaction?.timestamp || tx.ts || 0;
              const dismissedAt = getDismissedAt(from);
              if (!dismissedAt || txTs > dismissedAt) {
                invites.push({
                  from: hs.from || from,
                  ecdhPub: hs.ecdhPub,
                  hash: tx.hash,
                  txTs,
                });
              }
            }
          }
          if (hs.type === "dm_accept" && from !== myAddress) {
            const txTs =
              tx.timestamp || tx.transaction?.timestamp || tx.ts || 0;
            const session = getSession(from);
            if (
              session &&
              session.status === "pending_sent" &&
              txTs >= (session.createdAt || 0)
            ) {
              try {
                await completeHandshake(from, hs.ecdhPub);
                setSessions(loadSessions());
              } catch (e) {
                console.error("Failed to complete handshake:", e);
              }
            }
          }
        }
      }

      const finalSessions = loadSessions();
      const filteredInvites = invites.filter((inv) => {
        const s = finalSessions[inv.from];
        if (s && (s.status === "active" || s.status === "pending_sent"))
          return false;
        const dismissedAt = getDismissedAt(inv.from);
        if (dismissedAt && inv.txTs <= dismissedAt) return false;
        return true;
      });
      setPendingInvites(filteredInvites);

      if (activePeer) {
        const session = getSession(activePeer);
        if (session && session.status === "active") {
          interface PendingMsg {
            tx: any;
            env: EncryptedEnvelope;
            isSent: boolean;
            memo: string;
            from: string;
            to: string;
            ts: number;
            hash: string;
          }

          const resolvedMsgs: ChatMessage[] = [];
          const toDecrypt: PendingMsg[] = [];
          const sessionStart = session.createdAt || 0;

          for (const tx of txs) {
            const memo = tx.memo || tx.transaction?.memo;
            const from = tx.from || tx.transaction?.from;
            const to = tx.to || tx.transaction?.to;
            const ts = tx.timestamp || tx.transaction?.timestamp || tx.ts;
            const hash = tx.hash;
            if (!memo) continue;

            const env = parseEnvelope(memo);
            if (!env || !isDMEnvelope(env)) continue;

            const isSent = from === myAddress && to === activePeer;
            const isRecv = from === activePeer && to === myAddress;
            if (!isSent && !isRecv) continue;

            if (ts < sessionStart) continue;

            let plaintext: string | undefined;

            if (isSent) {
              plaintext =
                sentPlaintextsRef.current.get(hash) ||
                getCachedPlaintext(activePeer, hash);
              if (plaintext) {
                setCachedPlaintext(activePeer, hash, plaintext);
              }
            } else {
              plaintext =
                decryptedHashesRef.current.get(hash) ||
                getCachedPlaintext(activePeer, hash);
            }

            if (plaintext) {
              decryptedHashesRef.current.set(hash, plaintext);
              resolvedMsgs.push({
                hash,
                from,
                to,
                timestamp: ts,
                plaintext,
                encrypted: true,
                direction: isSent ? "sent" : "received",
                status: tx.status || tx.fast_path_status,
              });
            } else if (isRecv) {
              toDecrypt.push({
                tx,
                env: env as EncryptedEnvelope,
                isSent,
                memo,
                from,
                to,
                ts,
                hash,
              });
            } else {
              resolvedMsgs.push({
                hash,
                from,
                to,
                timestamp: ts,
                encrypted: true,
                direction: "sent",
                status: tx.status || tx.fast_path_status,
              });
            }
          }

          toDecrypt.sort((a, b) => a.env.n - b.env.n);

          for (const pending of toDecrypt) {
            let plaintext: string | undefined;
            let decryptError: string | undefined;
            try {
              plaintext = await decryptMessage(
                activePeer,
                pending.env,
                pending.hash,
              );
              if (plaintext) {
                decryptedHashesRef.current.set(pending.hash, plaintext);
              }
            } catch (e: any) {
              if (e.message?.includes("already processed")) {
                plaintext = undefined;
              } else {
                decryptError = e.message;
              }
            }
            resolvedMsgs.push({
              hash: pending.hash,
              from: pending.from,
              to: pending.to,
              timestamp: pending.ts,
              plaintext,
              encrypted: true,
              decryptError,
              direction: "received",
              status: pending.tx.status || pending.tx.fast_path_status,
            });
          }

          resolvedMsgs.sort((a, b) => a.timestamp - b.timestamp);
          if (resolvedMsgs.length > 0) {
            setMessages((prev) => {
              const chainMap = new Map(resolvedMsgs.map((m) => [m.hash, m]));
              const merged = prev.map((m) => {
                const chain = chainMap.get(m.hash);
                if (chain) {
                  chainMap.delete(m.hash);
                  if (
                    chain.status !== m.status ||
                    chain.plaintext !== m.plaintext
                  ) {
                    return {
                      ...m,
                      status: chain.status,
                      plaintext: chain.plaintext || m.plaintext,
                    };
                  }
                }
                return m;
              });
              const remaining = Array.from(chainMap.values());
              if (remaining.length === 0) {
                const changed = merged.some((m, i) => m !== prev[i]);
                return changed ? merged : prev;
              }
              return [...merged, ...remaining].sort(
                (a, b) => a.timestamp - b.timestamp,
              );
            });
          }
          if (toDecrypt.length > 0) {
            setSessions(loadSessions());
          }
        }
      }
    } catch (e) {
      console.error("Scan error:", e);
    }
  }, [wallet, myAddress, activePeer]);

  const { status: wsStatus, lastEvent: wsLastEvent } = useWebSocketContext();
  const lastChatRef = useRef(wsLastEvent);

  useEffect(() => {
    if (!wallet) return;
    scanForMessages();
  }, [wallet, scanForMessages]);

  useEffect(() => {
    if (!wallet || !wsLastEvent || wsLastEvent === lastChatRef.current) return;
    lastChatRef.current = wsLastEvent;
    if (wsLastEvent.type === 'FastPathExecuted' || wsLastEvent.type === 'NewTransaction') {
      scanForMessages();
    }
  }, [wallet, wsLastEvent, scanForMessages]);

  useEffect(() => {
    if (!wallet || wsStatus === 'connected') return;
    pollRef.current = setInterval(scanForMessages, 2000);
    return () => {
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, [wallet, wsStatus, scanForMessages]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const activeSession = activePeer ? getSession(activePeer) : null;
  const fsInfo = activePeer ? getForwardSecrecyInfo(activePeer) : null;

  if (!wallet) {
    return (
      <div className="tab-content chat-connect-prompt">
        <h3 className="chat-connect-title">encrypted chat</h3>
        <p className="chat-connect-desc">
          end-to-end encrypted messaging with forward secrecy via double ratchet
          protocol. messages are stored as transactions on the DAG.
        </p>
        <button onClick={onWalletOpen} className="chat-connect-btn">
          connect wallet to start
        </button>
      </div>
    );
  }

  return (
    <div className="tab-content" style={{ padding: "1rem" }}>
      <div className={`chat-container ${activePeer ? "has-active-peer" : ""}`}>
        {/* Sidebar */}
        <div className="chat-sidebar">
          <h4 className="chat-contacts-title">contacts</h4>

          {/* New chat */}
          <div className="chat-new-row">
            <input
              type="text"
              placeholder="peer address..."
              value={newPeerAddress}
              onChange={(e) => setNewPeerAddress(e.target.value)}
              className="chat-input"
              style={{ flex: 1, padding: "0.4rem", fontSize: "0.8rem" }}
            />
            <button
              onClick={startChat}
              disabled={loading}
              className="chat-add-btn"
            >
              +
            </button>
          </div>

          {/* Pending invites */}
          {pendingInvites.length > 0 && (
            <div className="chat-invite-section">
              <div className="chat-invite-label">
                pending invites ({pendingInvites.length})
              </div>
              {pendingInvites.map((inv) => (
                <div key={inv.from} className="chat-invite-card">
                  <div className="chat-contact-addr">
                    {inv.from.slice(0, 12)}...
                  </div>
                  <button
                    onClick={() => handleAcceptInvite(inv)}
                    disabled={loading}
                    className="chat-accept-btn"
                  >
                    accept
                  </button>
                </div>
              ))}
            </div>
          )}

          {/* Session list */}
          {Object.entries(sessions).map(([peer, session]) => (
            <div
              key={peer}
              onClick={() => {
                setActivePeer(peer);
                setMessages([]);
                decryptedHashesRef.current.clear();
              }}
              className={`chat-contact ${activePeer === peer ? "active" : ""}`}
            >
              <div className="chat-contact-inner">
                <span className="chat-contact-addr">
                  {peer.slice(0, 14)}...
                </span>
                <span
                  className={`chat-contact-status ${session.status === "active" ? "encrypted" : "awaiting"}`}
                >
                  {session.status === "active"
                    ? "encrypted"
                    : session.status === "pending_sent"
                      ? "awaiting"
                      : "pending"}
                </span>
              </div>
              {session.ratchet && (
                <div className="chat-contact-ratchet">
                  gen {session.ratchet.ratchetGeneration} ·{" "}
                  {(session.ratchet.totalSent || 0) +
                    (session.ratchet.totalReceived || 0)}{" "}
                  msgs
                </div>
              )}
            </div>
          ))}

          {Object.keys(sessions).length === 0 &&
            pendingInvites.length === 0 && (
              <div className="chat-empty">
                no conversations yet. enter an address above to start.
              </div>
            )}
        </div>

        {/* Chat area */}
        <div className="chat-main">
          {!activePeer ? (
            <div className="chat-no-peer">
              <h3 className="chat-connect-title" style={{ margin: 0 }}>
                encrypted chat
              </h3>
              <p className="chat-no-peer-desc">
                end-to-end encrypted DMs using the Double Ratchet protocol. each
                message uses a unique key derived from ECDH + HKDF. forward
                secrecy ensures past messages stay private even if keys are
                compromised.
              </p>
              <div className="chat-empty" style={{ textAlign: "center" }}>
                to start a chat, enter a peer address in the sidebar & click "+"
                to initiate a key exchange.
              </div>
            </div>
          ) : (
            <>
              {/* Chat header */}
              <div className="chat-header">
                <div>
                  <button
                    className="chat-back-btn"
                    onClick={() => {
                      setActivePeer(null);
                      setMessages([]);
                    }}
                  >
                    &larr;
                  </button>
                  <span className="chat-peer-header">
                    {activePeer.slice(0, 20)}...
                  </span>
                  {activeSession && (
                    <span
                      className={`chat-e2ee-status ${activeSession.status === "active" ? "active" : "pending"}`}
                      style={{ marginLeft: "0.5rem" }}
                    >
                      {activeSession.status === "active"
                        ? "E2EE active"
                        : "handshake pending"}
                    </span>
                  )}
                </div>
                <div className="chat-header-actions">
                  {fsInfo && (
                    <div className="chat-fs-info">
                      <span title="Ratchet generation - higher = more key rotations = stronger forward secrecy">
                        <span className="chat-e2ee-badge">ratchet</span> gen{" "}
                        {fsInfo.ratchetGeneration}
                      </span>
                      <span title="Messages sent with unique per-message keys">
                        <span className="chat-protocol-label">sent</span>{" "}
                        {fsInfo.messagesSent}
                      </span>
                      <span title="Messages received and decrypted">
                        <span className="chat-protocol-label">recv</span>{" "}
                        {fsInfo.messagesReceived}
                      </span>
                      <span title="Forward secrecy is active - each message key is derived and then destroyed">
                        <span className="chat-status-badge">FS</span>{" "}
                        {fsInfo.established ? "active" : "inactive"}
                      </span>
                    </div>
                  )}
                  <button
                    className="chat-end-btn"
                    onClick={() => {
                      if (
                        confirm(
                          "Delete this chat session? This will erase all keys.",
                        )
                      ) {
                        deleteSession(activePeer);
                        setSessions(loadSessions());
                        setActivePeer(null);
                        setMessages([]);
                      }
                    }}
                  >
                    end
                  </button>
                </div>
              </div>

              {/* Messages */}
              <div className="chat-messages">
                {activeSession?.status === "pending_sent" && (
                  <div className="chat-waiting">
                    waiting for peer to accept key exchange...
                    <br />
                    <span className="chat-waiting-hint">
                      a DH handshake transaction has been sent. once they
                      accept, encryption begins.
                    </span>
                  </div>
                )}

                {activeSession?.status === "active" &&
                  messages.length === 0 && (
                    <div className="chat-established">
                      E2EE session established. send a message!
                      <br />
                      <span className="chat-established-hint">
                        all messages are encrypted with AES-256-GCM using
                        per-message keys.
                      </span>
                    </div>
                  )}

                {messages.map((msg, i) => (
                  <div
                    key={msg.hash + i}
                    className={`chat-bubble ${msg.direction}`}
                  >
                    {msg.plaintext ? (
                      <div className="chat-bubble-text">{msg.plaintext}</div>
                    ) : msg.decryptError ? (
                      <div className="chat-decrypt-error">
                        decryption failed: {msg.decryptError}
                      </div>
                    ) : (
                      <div className="chat-encrypted-placeholder">
                        [encrypted message]
                      </div>
                    )}
                    <div className={`chat-bubble-meta ${msg.direction}`}>
                      <span>
                        {new Date(msg.timestamp).toLocaleTimeString()}
                      </span>
                      {msg.encrypted && (
                        <span className="chat-e2ee-badge">E2EE</span>
                      )}
                      {msg.status && (
                        <span className="chat-status-badge">{msg.status}</span>
                      )}
                    </div>
                  </div>
                ))}
                <div ref={messagesEndRef} />
              </div>

              {/* Input */}
              {activeSession?.status === "active" && (
                <div className="chat-input-area">
                  <div className="chat-input-wrap">
                    <input
                      type="text"
                      value={newMessage}
                      onChange={(e) =>
                        setNewMessage(e.target.value.slice(0, 80))
                      }
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && !e.shiftKey) {
                          e.preventDefault();
                          sendMessage();
                        }
                      }}
                      placeholder="type an encrypted message..."
                      maxLength={80}
                      className={`chat-input ${newMessage.length > 70 ? "near-limit" : ""}`}
                    />
                    <span
                      className={`chat-char-count ${newMessage.length > 70 ? "near-limit" : ""}`}
                    >
                      {newMessage.length}/80
                    </span>
                  </div>
                  <button
                    onClick={sendMessage}
                    disabled={loading || !newMessage.trim()}
                    className="chat-send-btn"
                  >
                    {loading ? "sending..." : "send"}
                  </button>
                </div>
              )}
            </>
          )}
        </div>
      </div>

      {/* Status messages */}
      {error && <div className="chat-error">{error}</div>}
      {success && <div className="chat-success">{success}</div>}

      {/* Protocol info footer */}
      <div className="chat-protocol-footer">
        <div>
          <span className="chat-protocol-label">protocol:</span> Double Ratchet
          (Signal-style)
        </div>
        <div>
          <span className="chat-protocol-label">key exchange:</span> ECDH P-256
        </div>
        <div>
          <span className="chat-protocol-label">encryption:</span> AES-256-GCM
        </div>
        <div>
          <span className="chat-protocol-label">key derivation:</span>{" "}
          HKDF-SHA256
        </div>
        <div>
          <span className="chat-protocol-label">transport:</span> DAG
          transaction (validator gossip)
        </div>
        <div>
          <span className="chat-protocol-label">forward secrecy:</span>{" "}
          per-message key derivation + deletion
        </div>
      </div>
    </div>
  );
}
