import { useState, useCallback, useEffect } from "react";
import type { ThreadTransaction, ThreadResponse } from "../types";
import { useSearchParams } from "react-router-dom";
import { useRinku } from "../context/WalletContext";
import { API_URL } from "../config";

interface Props {
  onWalletOpen: () => void;
}

export function ThreadTab({ onWalletOpen }: Props) {
  const { wallet, submitTransaction } = useRinku();
  const [searchParams, setSearchParams] = useSearchParams();
  const hashParam = searchParams.get("hash");
  const [mode, setMode] = useState<"create" | "load">(
    hashParam ? "load" : "create",
  );
  const [newThreadMemo, setNewThreadMemo] = useState("");
  const [viewThreadHash, setViewThreadHash] = useState(
    hashParam ? hashParam : "",
  );
  const [currentThread, setCurrentThread] = useState<ThreadTransaction | null>(
    null,
  );
  const [replies, setReplies] = useState<ThreadTransaction[]>([]);
  const [replyMemo, setReplyMemo] = useState("");
  const [replyTarget, setReplyTarget] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  useEffect(() => {
    if (hashParam) {
      setMode("load");
      setViewThreadHash(hashParam);
      loadThread(hashParam);
    }
  }, [hashParam]);

  const fetchAccount = async (address: string) => {
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
    setError(null);
    setSuccess(null);
    try {
      const tx = await fetchTransaction(hash);
      if (!tx) {
        setError("thread not found");
        setCurrentThread(null);
        setReplies([]);
        return;
      }
      setCurrentThread(tx);
      const repliesData = await fetchReplies(hash);
      setReplies(repliesData?.replies || []);
      setReplyTarget(hash);
    } catch {
      setError("failed to load");
    } finally {
      setLoading(false);
    }
  }, []);

  const crawlToRoot = async (hash: string) => {
    setLoading(true);
    setError(null);
    let current = await fetchTransaction(hash);
    if (!current) {
      setError("not found");
      setLoading(false);
      return;
    }
    while (current.references && current.references.length > 0) {
      const parent = await fetchTransaction(current.references[0]);
      if (!parent) break;
      current = parent;
    }
    setViewThreadHash(current.hash);
    await loadThread(current.hash);
    setLoading(false);
  };

  const createThread = async () => {
    if (!wallet) {
      onWalletOpen();
      return;
    }
    if (!newThreadMemo.trim() || newThreadMemo.length > 256) {
      setError("content required (max 256 chars)");
      return;
    }
    setLoading(true);
    setError(null);
    setSuccess(null);
    try {
      const result = await submitTransaction({
        to: wallet.fingerprint,
        amount: 0,
        kind: "transfer",
        memo: newThreadMemo.trim(),
      });
      if (result.hash) {
        setSuccess(result.hash);
        setNewThreadMemo("");
        setViewThreadHash(result.hash);
        setTimeout(() => loadThread(result.hash), 500);
      } else {
        setError("failed");
      }
    } catch (e) {
      setError(`${e}`);
    } finally {
      setLoading(false);
    }
  };

  const sendReply = async () => {
    if (!wallet) {
      onWalletOpen();
      return;
    }
    if (!replyTarget || !replyMemo.trim() || replyMemo.length > 256) {
      setError("reply content required (max 256)");
      return;
    }
    setLoading(true);
    setError(null);
    setSuccess(null);
    try {
      const result = await submitTransaction({
        to: wallet.fingerprint,
        amount: 0,
        kind: "transfer",
        memo: replyMemo.trim(),
        references: [replyTarget],
      });
      if (result.hash) {
        setSuccess(result.hash);
        setReplyMemo("");
        if (currentThread) {
          setTimeout(() => loadThread(currentThread.hash), 500);
        }
      } else {
        setError("failed");
      }
    } catch (e) {
      setError(`${e}`);
    } finally {
      setLoading(false);
    }
  };

  const StatusIcon = ({ tx }: { tx: ThreadTransaction }) => {
    const isConfirmed =
      tx.fast_path_status === "confirmed" ||
      tx.fast_path_status === "executed" ||
      tx.fast_path_status === "finalized";
    const isFinalized = tx.finalized || tx.fast_path_status === "finalized";
    const timeMs = tx.fast_path_finality_ms;

    return (
      <span className="thread-status-icons">
        {isConfirmed && (
          <span
            className="status-icon confirmed"
            title={timeMs ? `finalized in ${timeMs}ms` : "finalized"}
          >
            &#x2713;
          </span>
        )}
        {isFinalized && (
          <span className="status-icon finalized" title="anchored in snapshot">
            &#x25C6;
          </span>
        )}
        {!isConfirmed && !isFinalized && (
          <span className="status-icon pending" title="pending">
            &#x25CB;
          </span>
        )}
      </span>
    );
  };

  const ThreadPost = ({
    tx,
    depth = 0,
    isRoot = false,
  }: {
    tx: ThreadTransaction;
    depth?: number;
    isRoot?: boolean;
  }) => (
    <div
      className={`thread-node ${isRoot ? "root" : ""}`}
      style={{ marginLeft: depth * 16 }}
    >
      {depth > 0 && <div className="thread-line" />}
      <div className="thread-content">
        <div className="thread-header">
          <span className="thread-author">{tx.from.slice(0, 8)}</span>
          <StatusIcon tx={tx} />
        </div>
        <div className="thread-body">{tx.memo || "(empty)"}</div>
        <div className="thread-actions">
          <button
            className="thread-action"
            onClick={() => setReplyTarget(tx.hash)}
          >
            reply
          </button>
          <span className="thread-hash">{tx.hash.slice(0, 10)}</span>
        </div>
      </div>
    </div>
  );

  return (
    <div className="thread-tab">
      <div className="thread-mode-switcher">
        <button
          className={`mode-btn ${mode === "create" ? "active" : ""}`}
          onClick={() => setMode("create")}
        >
          create
        </button>
        <button
          className={`mode-btn ${mode === "load" ? "active" : ""}`}
          onClick={() => setMode("load")}
        >
          load
        </button>
      </div>

      {mode === "create" && (
        <div className="thread-box">
          <textarea
            className="thread-input-area"
            value={newThreadMemo}
            onChange={(e) => setNewThreadMemo(e.target.value)}
            placeholder="post something..."
            maxLength={256}
            rows={2}
          />
          <div className="thread-box-footer">
            <span className="char-count">{newThreadMemo.length}/256</span>
            <button
              className="thread-submit"
              onClick={createThread}
              disabled={loading || !newThreadMemo.trim()}
            >
              {loading ? "..." : "post"}
            </button>
          </div>
        </div>
      )}

      {mode === "load" && (
        <div className="thread-box">
          <div className="thread-load-row">
            <input
              className="thread-hash-input"
              type="text"
              value={viewThreadHash}
              onChange={(e) => setViewThreadHash(e.target.value)}
              placeholder="tx hash..."
            />
            <button
              className="thread-submit"
              onClick={() => loadThread(viewThreadHash)}
              disabled={loading}
            >
              view
            </button>
            <button
              className="thread-submit secondary"
              onClick={() => crawlToRoot(viewThreadHash)}
              disabled={loading}
            >
              root
            </button>
          </div>
        </div>
      )}

      {(error || success) && (
        <div className={`thread-message ${error ? "error" : "success"}`}>
          {error || (success && `created: ${success.slice(0, 12)}...`)}
        </div>
      )}

      {currentThread && (
        <div className="thread-view">
          <ThreadPost tx={currentThread} isRoot={true} />

          {replies.length > 0 && (
            <div className="thread-replies">
              {replies.map((reply) => (
                <ThreadPost key={reply.hash} tx={reply} depth={1} />
              ))}
            </div>
          )}

          {replies.length === 0 && (
            <div className="thread-empty">no replies yet</div>
          )}

          <div className="thread-reply-box">
            <div className="reply-target">
              replying to:{" "}
              <span className="reply-hash">
                {replyTarget?.slice(0, 10) || currentThread.hash.slice(0, 10)}
              </span>
              {replyTarget !== currentThread.hash && (
                <button
                  className="reply-reset"
                  onClick={() => setReplyTarget(currentThread.hash)}
                >
                  reset
                </button>
              )}
            </div>
            <textarea
              className="thread-input-area small"
              value={replyMemo}
              onChange={(e) => setReplyMemo(e.target.value)}
              placeholder="your reply..."
              maxLength={256}
              rows={1}
            />
            <div className="thread-box-footer">
              <span className="char-count">{replyMemo.length}/256</span>
              <button
                className="thread-submit"
                onClick={sendReply}
                disabled={loading || !replyMemo.trim()}
              >
                {loading ? "..." : "reply"}
              </button>
            </div>
          </div>
        </div>
      )}

      {!wallet && (
        <div className="thread-connect">
          <button onClick={onWalletOpen} className="thread-submit">
            connect wallet
          </button>
        </div>
      )}
    </div>
  );
}
