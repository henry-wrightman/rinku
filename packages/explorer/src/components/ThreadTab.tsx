import { useState, useCallback } from "react";
import type { ThreadTransaction, ThreadResponse } from "../types";
import type { SerializedKeyPair } from "../crypto";
import { createSignedTransaction } from "../crypto";

const getApiBaseUrl = () => {
  const envApiUrl = import.meta.env.VITE_API_URL;

  // If VITE_API_URL is set and not localhost, use it directly
  if (
    envApiUrl &&
    !envApiUrl.includes("127.0.0.1") &&
    !envApiUrl.includes("localhost")
  ) {
    console.log("Using VITE_API_URL:", envApiUrl);
    return `${envApiUrl}/api`;
  }
  return "/api";
};
const API_URL = getApiBaseUrl();

interface Props {
  wallet: SerializedKeyPair | null;
  onWalletOpen: () => void;
}

interface Account {
  fingerprint: string;
  balance: number;
  nonce: number;
  staked: number;
}

interface Message {
  type: "success" | "error";
  text: string;
}

export function ThreadTab({ wallet, onWalletOpen }: Props) {
  const [newThreadMemo, setNewThreadMemo] = useState("");
  const [viewThreadHash, setViewThreadHash] = useState("");
  const [replyToHash, setReplyToHash] = useState("");
  const [replyMemo, setReplyMemo] = useState("");
  const [currentThread, setCurrentThread] = useState<ThreadTransaction | null>(
    null,
  );
  const [replies, setReplies] = useState<ThreadTransaction[]>([]);
  const [loading, setLoading] = useState(false);
  const [message, setMessage] = useState<Message | null>(null);
  const [recentThreads, setRecentThreads] = useState<ThreadTransaction[]>([]);

  const fetchTips = async (): Promise<string[]> => {
    try {
      const res = await fetch(`${API_URL}/tips`);
      const data = await res.json();
      return data.tips || [];
    } catch {
      return [];
    }
  };

  const fetchAccount = async (address: string): Promise<Account | null> => {
    try {
      const res = await fetch(`${API_URL}/account/${address}`);
      if (!res.ok) return null;
      return res.json();
    } catch {
      return null;
    }
  };

  const fetchTransaction = async (
    hash: string,
  ): Promise<ThreadTransaction | null> => {
    try {
      const res = await fetch(`${API_URL}/tx/${hash}`);
      if (!res.ok) return null;
      return res.json();
    } catch {
      return null;
    }
  };

  const fetchReplies = async (hash: string): Promise<ThreadResponse | null> => {
    try {
      const res = await fetch(`${API_URL}/tx/${hash}/replies`);
      if (!res.ok) return null;
      return res.json();
    } catch {
      return null;
    }
  };

  const loadThread = useCallback(async (hash: string) => {
    if (!hash) return;
    setLoading(true);
    setMessage(null);

    try {
      const tx = await fetchTransaction(hash);
      console.log("tx", tx);
      if (!tx) {
        setMessage({
          type: "error",
          text: "Thread not found - it may not be confirmed yet. Please wait a moment and try again.",
        });
        setCurrentThread(null);
        setReplies([]);
        return;
      }

      setCurrentThread(tx);

      const repliesData = await fetchReplies(hash);
      setReplies(repliesData?.replies || []);
    } catch {
      setMessage({ type: "error", text: "Failed to load thread" });
    } finally {
      setLoading(false);
    }
  }, []);

  const crawlToRoot = async (
    hash: string,
  ): Promise<ThreadTransaction | null> => {
    let current = await fetchTransaction(hash);
    if (!current) return null;

    while (current.references && current.references.length > 0) {
      const parentHash = current.references[0];
      const parent = await fetchTransaction(parentHash);
      if (!parent) break;
      current = parent;
    }

    return current;
  };

  const handleViewThread = async () => {
    if (!viewThreadHash.trim()) {
      setMessage({ type: "error", text: "Please enter a transaction hash" });
      return;
    }
    await loadThread(viewThreadHash.trim());
  };

  const handleCrawlToRoot = async () => {
    if (!viewThreadHash.trim()) {
      setMessage({ type: "error", text: "Please enter a transaction hash" });
      return;
    }

    setLoading(true);
    setMessage(null);

    try {
      const root = await crawlToRoot(viewThreadHash.trim());
      if (root) {
        if (root.hash === viewThreadHash.trim()) {
          setMessage({
            type: "success",
            text: "This transaction is already the root (no references)",
          });
          await loadThread(root.hash);
        } else {
          setViewThreadHash(root.hash);
          await loadThread(root.hash);
          setMessage({
            type: "success",
            text: `Found root: ${root.hash.slice(0, 12)}...`,
          });
        }
      } else {
        setMessage({
          type: "error",
          text: "Transaction not found - it may not be confirmed yet. Please wait a moment and try again.",
        });
      }
    } catch {
      setMessage({ type: "error", text: "Failed to crawl to root" });
    } finally {
      setLoading(false);
    }
  };

  const createThread = async () => {
    if (!wallet) {
      onWalletOpen();
      return;
    }

    if (!newThreadMemo.trim()) {
      setMessage({
        type: "error",
        text: "Please enter some content for your thread",
      });
      return;
    }

    if (newThreadMemo.length > 256) {
      setMessage({
        type: "error",
        text: "Memo must be 256 characters or less",
      });
      return;
    }

    setLoading(true);
    setMessage(null);

    try {
      const address = wallet.fingerprint;
      const [tips, account] = await Promise.all([
        fetchTips(),
        fetchAccount(address),
      ]);

      const nonce = account ? account.nonce : 0;

      const signedTx = await createSignedTransaction(wallet, {
        to: address,
        amount: 0,
        nonce,
        parents: tips.slice(0, 8),
        kind: "transfer",
        gasPrice: 0.001,
        memo: newThreadMemo.trim(),
      });

      const res = await fetch(`${API_URL}/tx`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(signedTx),
      });

      const result = await res.json();

      if (res.ok && result.hash) {
        setMessage({
          type: "success",
          text: `Thread created: ${result.hash.slice(0, 12)}...`,
        });
        setNewThreadMemo("");
        setRecentThreads((prev) => [
          {
            hash: result.hash,
            from: signedTx.tx.from,
            to: signedTx.tx.to,
            amount: signedTx.tx.amount,
            nonce: signedTx.tx.nonce,
            ts: signedTx.tx.ts,
            parents: signedTx.tx.parents,
            sig: signedTx.tx.sig,
            fee: signedTx.tx.fee,
            memo: signedTx.tx.memo,
            references: signedTx.tx.references,
            finalized: false,
            weight: 1,
            url: `/tx/h/${result.hash}`,
            tipUrls: [],
          } as ThreadTransaction,
          ...prev.slice(0, 9),
        ]);
      } else {
        setMessage({
          type: "error",
          text: result.error || "Failed to create thread",
        });
      }
    } catch (e) {
      setMessage({ type: "error", text: `Error: ${e}` });
    } finally {
      setLoading(false);
    }
  };

  const replyToThread = async () => {
    if (!wallet) {
      onWalletOpen();
      return;
    }

    if (!replyToHash.trim()) {
      setMessage({ type: "error", text: "Please enter a hash to reply to" });
      return;
    }

    if (!replyMemo.trim()) {
      setMessage({ type: "error", text: "Please enter reply content" });
      return;
    }

    if (replyMemo.length > 256) {
      setMessage({
        type: "error",
        text: "Reply must be 256 characters or less",
      });
      return;
    }

    setLoading(true);
    setMessage(null);

    try {
      const address = wallet.fingerprint;
      const [tips, account] = await Promise.all([
        fetchTips(),
        fetchAccount(address),
      ]);

      const nonce = account ? account.nonce : 0;

      const signedTx = await createSignedTransaction(wallet, {
        to: address,
        amount: 0,
        nonce,
        parents: tips.slice(0, 8),
        kind: "transfer",
        gasPrice: 0.001,
        memo: replyMemo.trim(),
        references: [replyToHash.trim()],
      });

      const res = await fetch(`${API_URL}/tx`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(signedTx),
      });

      const result = await res.json();

      if (res.ok && result.hash) {
        setMessage({
          type: "success",
          text: `Reply sent: ${result.hash.slice(0, 12)}...`,
        });
        setReplyMemo("");
        if (currentThread && replyToHash === currentThread.hash) {
          await loadThread(currentThread.hash);
        }
      } else {
        setMessage({
          type: "error",
          text: result.error || "Failed to send reply",
        });
      }
    } catch (e) {
      setMessage({ type: "error", text: `Error: ${e}` });
    } finally {
      setLoading(false);
    }
  };

  const formatTime = (ts: number) => {
    const date = new Date(ts > 4000000000 ? ts : ts * 1000);
    return date.toLocaleString();
  };

  const formatHash = (hash: string) =>
    `${hash.slice(0, 8)}...${hash.slice(-6)}`;
  const formatAddress = (addr: string) =>
    `${addr.slice(0, 8)}...${addr.slice(-4)}`;

  return (
    <div className="rewards-tab">
      <div className="section">
        <h3>create new thread</h3>
        <p className="description">
          Start a new thread with a memo. This creates a transaction that can be
          replied to.
        </p>

        <div className="input-group">
          <textarea
            className="thread-textarea"
            value={newThreadMemo}
            onChange={(e) => setNewThreadMemo(e.target.value)}
            placeholder="What's on your mind? (max 256 chars)"
            maxLength={256}
            rows={3}
          />
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              marginTop: "8px",
            }}
          >
            <span className="char-count">{newThreadMemo.length}/256</span>
            <button
              onClick={createThread}
              disabled={loading || !newThreadMemo.trim()}
              className="btn primary"
            >
              {loading ? "posting..." : "post thread"}
            </button>
          </div>
        </div>
      </div>

      <div className="section">
        <h3>view thread</h3>
        <p className="description">
          Enter a transaction hash to view its thread and replies.
        </p>

        <div className="form-row">
          <input
            className="thread-input"
            type="text"
            value={viewThreadHash}
            onChange={(e) => setViewThreadHash(e.target.value)}
            placeholder="Transaction hash..."
          />
          <button
            onClick={handleViewThread}
            disabled={loading}
            className="btn primary"
          >
            view
          </button>
          <button
            onClick={handleCrawlToRoot}
            disabled={loading}
            className="btn"
          >
            find root
          </button>
        </div>
      </div>

      {message && (
        <div
          className={
            message.type === "success" ? "message-success" : "message-error"
          }
        >
          {message.text}
        </div>
      )}

      {currentThread && (
        <div className="section">
          <h3>thread</h3>

          <div className="thread-post">
            <div className="post-header">
              <span>{formatAddress(currentThread.from)}</span>
              <span>{formatTime(currentThread.ts)}</span>
            </div>
            <div className="post-content">
              {currentThread.memo || "(no content)"}
            </div>
            <div className="post-footer">
              <span className="hash">{formatHash(currentThread.hash)}</span>
              <span
                className={
                  currentThread.fast_path_status === 'confirmed' || currentThread.finalized
                    ? "status-finalized"
                    : "status-pending"
                }
              >
                {currentThread.fast_path_status === 'confirmed' 
                  ? `confirmed${currentThread.fast_path_finality_ms ? ` (${currentThread.fast_path_finality_ms}ms)` : ''}`
                  : currentThread.finalized ? "finalized" : "pending"}
              </span>
            </div>
            {currentThread.references &&
              currentThread.references.length > 0 && (
                <div style={{ marginTop: "8px", fontSize: "0.85em" }}>
                  replies to:{" "}
                  {currentThread.references.map((r) => (
                    <button
                      key={r}
                      className="link-btn"
                      onClick={() => {
                        setViewThreadHash(r);
                        loadThread(r);
                      }}
                    >
                      {formatHash(r)}
                    </button>
                  ))}
                </div>
              )}
          </div>

          <div className="reply-form">
            <h4>reply to this thread</h4>
            <div className="form-row">
              <input
                className="thread-input"
                type="text"
                value={replyToHash}
                onChange={(e) => setReplyToHash(e.target.value)}
                placeholder="Hash to reply to..."
              />
              <button
                onClick={() => setReplyToHash(currentThread.hash)}
                className="btn"
              >
                use current
              </button>
            </div>
            <textarea
              className="thread-textarea"
              value={replyMemo}
              onChange={(e) => setReplyMemo(e.target.value)}
              placeholder="Your reply... (max 256 chars)"
              maxLength={256}
              rows={2}
            />
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                marginTop: "8px",
              }}
            >
              <span className="char-count">{replyMemo.length}/256</span>
              <button
                onClick={replyToThread}
                disabled={loading || !replyMemo.trim() || !replyToHash.trim()}
                className="btn primary"
              >
                {loading ? "sending..." : "send reply"}
              </button>
            </div>
          </div>

          {replies.length > 0 && (
            <div className="replies">
              <h4>replies ({replies.length})</h4>
              {replies.map((reply) => (
                <div key={reply.hash} className="reply-post">
                  <div className="post-header">
                    <span>{formatAddress(reply.from)}</span>
                    <span>{formatTime(reply.ts)}</span>
                  </div>
                  <div className="post-content">
                    {reply.memo || "(no content)"}
                  </div>
                  <div className="post-footer">
                    <button
                      className="link-btn"
                      onClick={() => {
                        setViewThreadHash(reply.hash);
                        loadThread(reply.hash);
                      }}
                    >
                      {formatHash(reply.hash)}
                    </button>
                    <div>
                      <button
                        className="link-btn"
                        onClick={() => setReplyToHash(reply.hash)}
                        style={{ marginRight: "8px" }}
                      >
                        reply
                      </button>
                      <span
                        className={
                          reply.fast_path_status === 'confirmed' || reply.finalized
                            ? "status-finalized"
                            : "status-pending"
                        }
                      >
                        {reply.fast_path_status === 'confirmed' 
                          ? `confirmed${reply.fast_path_finality_ms ? ` (${reply.fast_path_finality_ms}ms)` : ''}`
                          : reply.finalized ? "finalized" : "pending"}
                      </span>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          )}

          {replies.length === 0 && (
            <div style={{ textAlign: "center", opacity: 0.6, padding: "24px" }}>
              No replies yet. Be the first to reply!
            </div>
          )}
        </div>
      )}

      {recentThreads.length > 0 && (
        <div className="section">
          <h3>your recent threads</h3>
          {recentThreads.map((thread) => (
            <div
              key={thread.hash}
              className="recent-thread"
              onClick={() => {
                setViewThreadHash(thread.hash);
                loadThread(thread.hash);
              }}
            >
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  marginBottom: "4px",
                }}
              >
                <span className="hash">{formatHash(thread.hash)}</span>
                <span style={{ fontSize: "0.85em", opacity: 0.7 }}>
                  {formatTime(thread.ts)}
                </span>
              </div>
              <div
                style={{
                  opacity: 0.9,
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                }}
              >
                {thread.memo || "(no content)"}
              </div>
            </div>
          ))}
        </div>
      )}

      {!wallet && (
        <div className="section" style={{ textAlign: "center" }}>
          <p style={{ marginBottom: "16px", opacity: 0.8 }}>
            Connect your wallet to create threads and replies.
          </p>
          <button onClick={onWalletOpen} className="btn primary">
            connect wallet
          </button>
        </div>
      )}
    </div>
  );
}
