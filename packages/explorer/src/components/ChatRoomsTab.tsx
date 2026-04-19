import { useState, useEffect, useRef, useCallback } from "react";
import { useRinku } from "../context/WalletContext";
import { API_URL } from "../config";
import { useWebSocketContext } from "../context/WebSocketContext";

const NODE_URL = API_URL;

interface RoomMeta {
  id: string;
  name: string;
  owner: string;
  created_at: number;
  member_count: number;
  message_count: number;
  capacity: number;
}

interface ChatMessage {
  seq: number;
  author: string;
  ts: number;
  content: string;
}

interface ContractInfo {
  contractId: string;
  state?: Record<string, unknown>;
}

interface Props {
  onWalletOpen: () => void;
}

export function ChatRoomsTab({ onWalletOpen }: Props) {
  const { wallet: keyPair, accountInfo, submitTransaction } = useRinku();
  const walletReady = !!keyPair;

  const [contractId, setContractId] = useState<string>(() => {
    return localStorage.getItem("rinku_chat_contract") || "";
  });
  const [contractInput, setContractInput] = useState("");
  const [deploying, setDeploying] = useState(false);
  const [deployError, setDeployError] = useState<string | null>(null);

  const [rooms, setRooms] = useState<RoomMeta[]>([]);
  const [activeRoom, setActiveRoom] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [members, setMembers] = useState<string[]>([]);
  const [roomMeta, setRoomMeta] = useState<RoomMeta | null>(null);

  const [newRoomId, setNewRoomId] = useState("");
  const [newRoomName, setNewRoomName] = useState("");
  const [messageInput, setMessageInput] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [optimisticJoins, setOptimisticJoins] = useState<Set<string>>(
    new Set(),
  );
  const [optimisticMessagesByRoom, setOptimisticMessagesByRoom] = useState<
    Record<string, ChatMessage[]>
  >({});
  const [optimisticRooms, setOptimisticRooms] = useState<RoomMeta[]>([]);
  const [joinRoomInput, setJoinRoomInput] = useState("");
  const [systemEvents, setSystemEvents] = useState<ChatMessage[]>([]);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const optimisticSeqRef = useRef(-1);
  const prevMembersRef = useRef<string[]>([]);

  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const scrollToBottom = () => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  };

  useEffect(() => {
    scrollToBottom();
  }, [messages, optimisticMessagesByRoom, activeRoom, systemEvents]);

  const fetchContractState = useCallback(async () => {
    if (!contractId) return;
    try {
      const res = await fetch(`${NODE_URL}/contracts/${contractId}`);
      if (!res.ok) return;
      const data: ContractInfo = await res.json();
      if (data.state) {
        const roomIds = data.state["rooms:all"] as string[] | undefined;
        if (roomIds) {
          const roomList: RoomMeta[] = [];
          for (const rid of roomIds) {
            const meta = data.state[`room:${rid}:meta`] as RoomMeta | undefined;
            if (meta) roomList.push(meta);
          }
          setRooms(roomList);
          setOptimisticRooms((prev) =>
            prev.filter((r) => !roomIds.includes(r.id)),
          );
        }

        if (activeRoom) {
          const meta = data.state[`room:${activeRoom}:meta`] as
            | RoomMeta
            | undefined;
          if (meta) {
            setRoomMeta(meta);
            const memberList = data.state[`room:${activeRoom}:members`] as
              | string[]
              | undefined;
            if (memberList) {
              const prev = prevMembersRef.current;
              if (prev.length > 0) {
                const left = prev.filter((m) => !memberList.includes(m));
                const joined = memberList.filter((m) => !prev.includes(m));
                const now = Math.floor(Date.now() / 1000);
                const newEvents: ChatMessage[] = [];
                for (const addr of left) {
                  newEvents.push({
                    seq: -100000 - now,
                    author: "system",
                    ts: now,
                    content: `${addr.slice(0, 10)}... left the room`,
                  });
                }
                for (const addr of joined) {
                  if (
                    addr !== keyPair?.fingerprint ||
                    !optimisticJoins.has(activeRoom)
                  ) {
                    newEvents.push({
                      seq: -100000 - now - 1,
                      author: "system",
                      ts: now,
                      content: `${addr.slice(0, 10)}... joined the room`,
                    });
                  }
                }
                if (newEvents.length > 0) {
                  setSystemEvents((prev) => [...prev.slice(-20), ...newEvents]);
                }
              }
              prevMembersRef.current = memberList;
              setMembers(memberList);
            }

            const msgs: ChatMessage[] = [];
            const count = meta.message_count;
            const cap = meta.capacity;
            const fetchCount = Math.min(20, count, cap);
            const startSeq = count > fetchCount ? count - fetchCount : 0;
            for (let seq = startSeq; seq < count; seq++) {
              const slot = seq % cap;
              const msg = data.state[`room:${activeRoom}:msg:${slot}`] as
                | ChatMessage
                | undefined;
              if (msg) msgs.push(msg);
            }
            setMessages(msgs);
            if (activeRoom) {
              setOptimisticMessagesByRoom((prev) => {
                const roomMsgs = prev[activeRoom] || [];
                if (roomMsgs.length === 0) return prev;
                const now = Math.floor(Date.now() / 1000);
                const filtered = roomMsgs.filter((om) => {
                  const matchByContent = msgs.some(
                    (m) => m.content === om.content,
                  );
                  const tooOld = now - om.ts > 30;
                  return !matchByContent && !tooOld;
                });
                if (filtered.length === 0) {
                  const { [activeRoom]: _, ...rest } = prev;
                  return rest;
                }
                return { ...prev, [activeRoom]: filtered };
              });
            }
          }
        }
      }
    } catch (e) {
      console.error("Failed to fetch contract state:", e);
    }
  }, [contractId, activeRoom]);

  const { status: wsStatus, lastBatch } = useWebSocketContext();
  const lastBatchIdRef = useRef(0);
  const chatRoomRefreshRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!contractId) return;
    fetchContractState();
    return () => {
      if (chatRoomRefreshRef.current) clearTimeout(chatRoomRefreshRef.current);
    };
  }, [contractId, fetchContractState]);

  useEffect(() => {
    if (!contractId || !lastBatch || lastBatch.id === lastBatchIdRef.current)
      return;
    lastBatchIdRef.current = lastBatch.id;
    const relevant = lastBatch.items.some(
      (e) => e.type === "CheckpointCreated",
    );
    if (relevant && !chatRoomRefreshRef.current) {
      chatRoomRefreshRef.current = setTimeout(() => {
        chatRoomRefreshRef.current = null;
        fetchContractState();
      }, 500);
    }
  }, [lastBatch, contractId, fetchContractState]);

  useEffect(() => {
    if (!contractId || wsStatus === "connected") return;
    const interval = setInterval(fetchContractState, 3000);
    return () => clearInterval(interval);
  }, [wsStatus, contractId, fetchContractState]);

  const callContract = async (
    entrypoint: string,
    input: Record<string, unknown>,
  ) => {
    if (!walletReady || !keyPair || !contractId) return null;
    setError(null);
    setSubmitting(true);
    try {
      const contractData = JSON.stringify({
        action: "call",
        contractId,
        entrypoint,
        input,
      });
      const result = await submitTransaction({
        to: contractId,
        amount: 0,
        kind: "contract",
        data: contractData,
      });
      setTimeout(() => fetchContractState(), 200);
      setTimeout(() => fetchContractState(), 1500);
      return result;
    } catch (e: any) {
      setError(e.message);
      return null;
    } finally {
      setSubmitting(false);
    }
  };

  const handleDeployChat = async () => {
    if (!walletReady || !keyPair) {
      onWalletOpen();
      return;
    }
    setDeploying(true);
    setDeployError(null);
    try {
      const wasmRes = await fetch("/contracts/rinku_chat_contract.wasm");
      if (!wasmRes.ok) throw new Error("failed to fetch chat contract wasm");
      const wasmBuffer = await wasmRes.arrayBuffer();
      const bytes = new Uint8Array(wasmBuffer);
      let binary = "";
      for (let i = 0; i < bytes.length; i++) {
        binary += String.fromCharCode(bytes[i]);
      }
      const wasmBase64 = btoa(binary);

      const contractData = JSON.stringify({
        action: "deploy",
        wasmBase64,
        initState: {},
      });

      const result = await submitTransaction({
        to: "contract:deploy",
        amount: 0,
        kind: "contract",
        data: contractData,
      });

      const txHash = result.hash;
      setStatus(
        `chat contract deployed! tx: ${txHash.slice(0, 16)}... (waiting for finalization...)`,
      );

      const pollForContract = async (attempts: number) => {
        for (let i = 0; i < attempts; i++) {
          await new Promise((r) => setTimeout(r, 3000));
          try {
            const res = await fetch(`${NODE_URL}/contracts`);
            const data = await res.json();
            const contracts = data.contracts || [];
            const found = contracts.find(
              (c: any) => c.creator === keyPair.fingerprint,
            );
            if (found) {
              setContractId(found.contractId);
              localStorage.setItem("rinku_chat_contract", found.contractId);
              setStatus(
                `connected to chat contract: ${found.contractId.slice(0, 16)}...`,
              );
              return;
            }
          } catch {}
        }
        setStatus(
          `deploy tx submitted. enter the contract id manually after finalization.`,
        );
      };
      pollForContract(10);
    } catch (e: any) {
      setDeployError(e.message);
    } finally {
      setDeploying(false);
    }
  };

  const handleSetContract = () => {
    if (contractInput.trim()) {
      setContractId(contractInput.trim());
      localStorage.setItem("rinku_chat_contract", contractInput.trim());
      setContractInput("");
    }
  };

  const handleCreateRoom = async () => {
    if (!newRoomId.trim()) return;
    const result = await callContract("create_room", {
      id: newRoomId.trim().toLowerCase().replace(/\s+/g, "-"),
      name: newRoomName.trim() || newRoomId.trim(),
    });
    if (result) {
      const roomId = newRoomId.trim().toLowerCase().replace(/\s+/g, "-");
      const roomName = newRoomName.trim() || newRoomId.trim();
      setNewRoomId("");
      setNewRoomName("");
      setStatus(`room created!`);
      setActiveRoom(roomId);
      setSidebarOpen(false);
      setOptimisticJoins((prev) => new Set(prev).add(roomId));
      setOptimisticRooms((prev) => [
        ...prev,
        {
          id: roomId,
          name: roomName,
          owner: keyPair!.fingerprint,
          created_at: Math.floor(Date.now() / 1000),
          member_count: 1,
          message_count: 0,
          capacity: 50,
        },
      ]);
    }
  };

  const handleJoinRoom = async (roomId: string) => {
    const result = await callContract("join_room", { id: roomId });
    if (result) {
      setActiveRoom(roomId);
      setOptimisticJoins((prev) => new Set(prev).add(roomId));
      setStatus(`joined #${roomId}`);
    }
  };

  const handleLeaveRoom = async (roomId: string) => {
    await callContract("leave_room", { id: roomId });
    setOptimisticJoins((prev) => {
      const next = new Set(prev);
      next.delete(roomId);
      return next;
    });
    if (activeRoom === roomId) {
      setActiveRoom(null);
      setMessages([]);
      setMembers([]);
      setRoomMeta(null);
      setSystemEvents([]);
      prevMembersRef.current = [];
    }
    setStatus(`left #${roomId}`);
  };

  const handleSendMessage = async () => {
    if (!messageInput.trim() || !activeRoom || !keyPair) return;
    const content = messageInput.trim();
    setMessageInput("");
    const seq = optimisticSeqRef.current--;
    const roomId = activeRoom;
    setOptimisticMessagesByRoom((prev) => ({
      ...prev,
      [roomId]: [
        ...(prev[roomId] || []),
        {
          seq,
          author: keyPair.fingerprint,
          ts: Math.floor(Date.now() / 1000),
          content,
        },
      ],
    }));
    const result = await callContract("send_message", {
      id: roomId,
      content,
    });
    if (!result) {
      setOptimisticMessagesByRoom((prev) => ({
        ...prev,
        [roomId]: (prev[roomId] || []).filter((m) => m.seq !== seq),
      }));
    }
    inputRef.current?.focus();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSendMessage();
    }
  };

  const handleJoinRoomById = async () => {
    const roomId = joinRoomInput.trim().toLowerCase().replace(/\s+/g, "-");
    if (!roomId) return;
    setJoinRoomInput("");
    const result = await callContract("join_room", { id: roomId });
    if (result) {
      setActiveRoom(roomId);
      setSidebarOpen(false);
      setOptimisticJoins((prev) => new Set(prev).add(roomId));
      setStatus(`joined #${roomId}`);
    }
  };

  const isMember = (roomId: string) => {
    if (!keyPair) return false;
    if (optimisticJoins.has(roomId)) return true;
    if (activeRoom === roomId) return members.includes(keyPair.fingerprint);
    return false;
  };

  const isActiveMember =
    activeRoom &&
    (members.includes(keyPair?.fingerprint || "") ||
      optimisticJoins.has(activeRoom));

  const allRooms = [
    ...rooms,
    ...optimisticRooms.filter((or) => !rooms.some((r) => r.id === or.id)),
  ];

  const optimisticMessages = activeRoom
    ? optimisticMessagesByRoom[activeRoom] || []
    : [];
  const allMessages = [
    ...messages,
    ...systemEvents,
    ...optimisticMessages.filter(
      (om) =>
        !messages.some(
          (m) => m.content === om.content && m.author === om.author,
        ),
    ),
  ].sort((a, b) => {
    const aReal = a.seq >= 0 ? a.seq : a.ts;
    const bReal = b.seq >= 0 ? b.seq : b.ts;
    if (a.seq >= 0 && b.seq >= 0) return a.seq - b.seq;
    if (a.seq >= 0 && b.seq < 0) return -1;
    if (a.seq < 0 && b.seq >= 0) return 1;
    return aReal - bReal;
  });

  const formatTime = (ts: number) => {
    const d = new Date(ts * 1000);
    return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  };

  const formatAddr = (addr: string) => addr.slice(0, 8) + "...";

  if (!contractId) {
    return (
      <div className="cr-tab">
        <div className="section">
          <h3>chat rooms</h3>
          <p className="cr-description">
            IRC-like chat rooms powered by a Rinku smart contract. Deploy a new
            chat contract or connect to an existing one.
          </p>

          {(deployError || status) && (
            <div className="cr-status-bar">
              {deployError && <div className="error">{deployError}</div>}
              {status && <div className="success">{status}</div>}
            </div>
          )}

          <div className="cr-deploy-actions">
            <button
              onClick={handleDeployChat}
              disabled={deploying || !walletReady}
            >
              {deploying ? "deploying..." : "deploy chat contract"}
            </button>
            {!walletReady && (
              <span className="cr-connect-hint">
                <span onClick={onWalletOpen} className="cr-connect-link">
                  connect wallet
                </span>{" "}
                first
              </span>
            )}
          </div>

          <div className="cr-contract-input-section">
            <label className="cr-label">or enter existing contract id</label>
            <div className="cr-contract-input-row">
              <input
                type="text"
                placeholder="contract_abc123..."
                value={contractInput}
                onChange={(e) => setContractInput(e.target.value)}
                className="cr-input"
              />
              <button
                onClick={handleSetContract}
                disabled={!contractInput.trim()}
              >
                connect
              </button>
            </div>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="cr-tab">
      <div className="cr-layout">
        <button
          className={`cr-sidebar-toggle ${sidebarOpen ? "open" : ""}`}
          onClick={() => setSidebarOpen(!sidebarOpen)}
        >
          {sidebarOpen ? "close" : activeRoom ? `#${activeRoom}` : "rooms"}
          <span className="cr-toggle-icon">
            {sidebarOpen ? "\u2715" : "\u2630"}
          </span>
        </button>

        <div className={`cr-sidebar ${sidebarOpen ? "open" : ""}`}>
          <div className="cr-sidebar-header">
            <span className="cr-accent">contract:</span>{" "}
            <span className="mono cr-contract-id">
              {contractId.slice(0, 12)}...
            </span>
            <span
              onClick={() => {
                setContractId("");
                localStorage.removeItem("rinku_chat_contract");
              }}
              className="cr-disconnect"
            >
              disconnect
            </span>
          </div>

          <div className="cr-sidebar-section">
            <div className="cr-sidebar-label">rooms</div>
            {allRooms.length === 0 ? (
              <div className="cr-empty-rooms">no rooms yet</div>
            ) : (
              allRooms.map((room) => {
                const isOptimistic =
                  optimisticRooms.some((r) => r.id === room.id) &&
                  !rooms.some((r) => r.id === room.id);
                return (
                  <div
                    key={room.id}
                    onClick={() => {
                      setActiveRoom(room.id);
                      setMessages([]);
                      setMembers([]);
                      setSystemEvents([]);
                      prevMembersRef.current = [];
                      setSidebarOpen(false);
                    }}
                    className={`cr-room-item ${activeRoom === room.id ? "active" : ""} ${isOptimistic ? "optimistic" : ""}`}
                  >
                    <span>#{room.id}</span>
                    <span className="cr-room-meta">
                      {isOptimistic ? (
                        <span className="cr-spinner" aria-label="pending" />
                      ) : (
                        `${room.member_count}u ${room.message_count}m`
                      )}
                    </span>
                  </div>
                );
              })
            )}
          </div>

          {walletReady && (
            <div className="cr-sidebar-section">
              <div className="cr-sidebar-label">create room</div>
              <input
                type="text"
                placeholder="room-id"
                value={newRoomId}
                onChange={(e) => setNewRoomId(e.target.value)}
                className="cr-sidebar-input"
              />
              <input
                type="text"
                placeholder="display name (optional)"
                value={newRoomName}
                onChange={(e) => setNewRoomName(e.target.value)}
                className="cr-sidebar-input"
              />
              <button
                onClick={handleCreateRoom}
                disabled={submitting || !newRoomId.trim()}
                className="cr-sidebar-btn"
              >
                {submitting ? "..." : "create"}
              </button>
            </div>
          )}

          {walletReady && (
            <div className="cr-sidebar-section">
              <div className="cr-sidebar-label">join room</div>
              <div className="cr-join-row">
                <input
                  type="text"
                  placeholder="room-id"
                  value={joinRoomInput}
                  onChange={(e) => setJoinRoomInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleJoinRoomById();
                  }}
                  className="cr-sidebar-input"
                />
                <button
                  onClick={handleJoinRoomById}
                  disabled={submitting || !joinRoomInput.trim()}
                  className="cr-join-btn"
                >
                  join
                </button>
              </div>
            </div>
          )}

          {!walletReady && (
            <div className="cr-sidebar-section">
              <span onClick={onWalletOpen} className="cr-connect-link">
                connect wallet
              </span>
            </div>
          )}
        </div>

        {sidebarOpen && (
          <div
            className="cr-sidebar-overlay"
            onClick={() => setSidebarOpen(false)}
          />
        )}

        <div className="cr-main">
          {!activeRoom ? (
            <div className="cr-placeholder">select a room or create one</div>
          ) : (
            <>
              <div className="cr-room-header">
                <div>
                  <span className="cr-room-title">#{activeRoom}</span>
                  {roomMeta && (
                    <span className="cr-room-info">
                      {roomMeta.member_count} members | {roomMeta.message_count}{" "}
                      messages
                    </span>
                  )}
                </div>
                <div className="cr-room-actions">
                  {walletReady && !isActiveMember && (
                    <button
                      onClick={() => handleJoinRoom(activeRoom)}
                      disabled={submitting}
                      className="cr-action-btn"
                    >
                      join
                    </button>
                  )}
                  {walletReady && isActiveMember && (
                    <button
                      onClick={() => handleLeaveRoom(activeRoom)}
                      disabled={submitting}
                      className="cr-action-btn cr-leave-btn"
                    >
                      leave
                    </button>
                  )}
                </div>
              </div>

              {(error || status) && (
                <div className="cr-inline-status">
                  {error && <span className="cr-error-text">{error}</span>}
                  {status && <span className="cr-success-text">{status}</span>}
                </div>
              )}

              <div className="cr-messages">
                {allMessages.length === 0 ? (
                  <div className="cr-placeholder">
                    {isActiveMember
                      ? "no messages yet. say something!"
                      : "join the room to chat"}
                  </div>
                ) : (
                  allMessages.map((msg, idx) => {
                    const isSystem = msg.author === "system";
                    const isMe = msg.author === keyPair?.fingerprint;
                    const isPending = msg.seq < 0 && !isSystem;
                    if (isSystem) {
                      return (
                        <div key={`sys-${idx}`} className="cr-system-msg">
                          — {msg.content} —
                        </div>
                      );
                    }
                    return (
                      <div
                        key={msg.seq}
                        className={`cr-msg ${isPending ? "pending" : ""}`}
                      >
                        <span className="cr-msg-time">
                          {formatTime(msg.ts)}
                        </span>
                        <span className={`cr-msg-author ${isMe ? "me" : ""}`}>
                          {isMe ? "you" : formatAddr(msg.author)}
                        </span>
                        <span className="cr-msg-content">{msg.content}</span>
                        {isPending && (
                          <span className="cr-msg-pending">
                            <span className="cr-spinner" aria-label="pending" />
                          </span>
                        )}
                      </div>
                    );
                  })
                )}
                <div ref={messagesEndRef} />
              </div>

              {walletReady && isActiveMember && (
                <div className="cr-input-bar">
                  <input
                    ref={inputRef}
                    type="text"
                    placeholder="type a message..."
                    value={messageInput}
                    onChange={(e) => setMessageInput(e.target.value)}
                    onKeyDown={handleKeyDown}
                    disabled={submitting}
                    className="cr-message-input"
                  />
                  <button
                    onClick={handleSendMessage}
                    disabled={submitting || !messageInput.trim()}
                    className="cr-send-btn"
                  >
                    {submitting ? "..." : "send"}
                  </button>
                </div>
              )}

              {members.length > 0 && (
                <div className="cr-members-bar">
                  members:{" "}
                  {members
                    .map((m) =>
                      m === keyPair?.fingerprint ? "you" : formatAddr(m),
                    )
                    .join(", ")}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
