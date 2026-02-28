import {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  type ReactNode,
} from 'react';
import {
  useWebSocket,
  type NodeEvent,
  type NodeEventType,
  type ConnectionStatus,
} from '../hooks/useWebSocket';

interface WebSocketSummary {
  lastCheckpointHeight: number | null;
  recentTxCount: number;
}

interface WebSocketContextType {
  status: ConnectionStatus;
  lastEvent: NodeEvent | null;
  events: NodeEvent[];
  summary: WebSocketSummary;
  send: (message: string) => void;
}

const WebSocketContext = createContext<WebSocketContextType | null>(null);

export function WebSocketProvider({ children }: { children: ReactNode }) {
  const { status, lastEvent, events, send } = useWebSocket();
  const [summary, setSummary] = useState<WebSocketSummary>({
    lastCheckpointHeight: null,
    recentTxCount: 0,
  });

  useEffect(() => {
    if (!lastEvent) return;

    setSummary((prev) => {
      const next = { ...prev };

      if (lastEvent.type === 'CheckpointCreated') {
        const data = lastEvent.data as { height: number };
        next.lastCheckpointHeight = data.height;
      }

      if (
        lastEvent.type === 'NewTransaction' ||
        lastEvent.type === 'FastPathConfirmed' ||
        lastEvent.type === 'FastPathExecuted'
      ) {
        next.recentTxCount = prev.recentTxCount + 1;
      }

      return next;
    });
  }, [lastEvent]);

  return (
    <WebSocketContext.Provider value={{ status, lastEvent, events, summary, send }}>
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
  const { lastEvent } = useWebSocketContext();
  const [matched, setMatched] = useState<NodeEvent | null>(null);

  useEffect(() => {
    if (lastEvent && lastEvent.type === type) {
      setMatched(lastEvent);
    }
  }, [lastEvent, type]);

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
