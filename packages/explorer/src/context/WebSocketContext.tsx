import {
  createContext,
  useContext,
  useState,
  useEffect,
  useRef,
  type ReactNode,
} from 'react';
import {
  useWebSocket,
  type NodeEvent,
  type NodeEventType,
  type EventBatch,
  type ConnectionStatus,
} from '../hooks/useWebSocket';

interface WebSocketSummary {
  lastCheckpointHeight: number | null;
  recentTxCount: number;
}

interface WebSocketContextType {
  status: ConnectionStatus;
  lastBatch: EventBatch | null;
  events: NodeEvent[];
  summary: WebSocketSummary;
  send: (message: string) => void;
}

const WebSocketContext = createContext<WebSocketContextType | null>(null);

export function WebSocketProvider({ children }: { children: ReactNode }) {
  const { status, lastBatch, events, send } = useWebSocket();
  const [summary, setSummary] = useState<WebSocketSummary>({
    lastCheckpointHeight: null,
    recentTxCount: 0,
  });

  useEffect(() => {
    if (!lastBatch) return;

    setSummary((prev) => {
      const next = { ...prev };

      for (const evt of lastBatch.items) {
        if (evt.type === 'CheckpointCreated') {
          const data = evt.data as { height: number };
          next.lastCheckpointHeight = data.height;
        }

        if (
          evt.type === 'NewTransaction' ||
          evt.type === 'FastPathConfirmed' ||
          evt.type === 'FastPathExecuted'
        ) {
          next.recentTxCount = prev.recentTxCount + 1;
        }
      }

      return next;
    });
  }, [lastBatch]);

  return (
    <WebSocketContext.Provider value={{ status, lastBatch, events, summary, send }}>
      {children}
    </WebSocketContext.Provider>
  );
}

export function useWebSocketContext(): WebSocketContextType {
  const ctx = useContext(WebSocketContext);
  if (!ctx) {
    throw new Error('useWebSocketContext must be used within a WebSocketProvider');
  }
  return ctx;
}

export function useNodeEvent(type: NodeEventType): NodeEvent | null {
  const { lastBatch } = useWebSocketContext();
  const [matched, setMatched] = useState<NodeEvent | null>(null);
  const lastBatchIdRef = useRef<number>(0);

  useEffect(() => {
    if (!lastBatch || lastBatch.id === lastBatchIdRef.current) return;
    lastBatchIdRef.current = lastBatch.id;
    const found = lastBatch.items.find((e) => e.type === type);
    if (found) {
      setMatched(found);
    }
  }, [lastBatch, type]);

  return matched;
}

export function useNodeEvents(types: NodeEventType[]): NodeEvent[] {
  const { events } = useWebSocketContext();
  const [filtered, setFiltered] = useState<NodeEvent[]>([]);

  useEffect(() => {
    setFiltered(events.filter((e) => types.includes(e.type)));
  }, [events, types]);

  return filtered;
}

export function useWsStatus(): ConnectionStatus {
  const { status } = useWebSocketContext();
  return status;
}
