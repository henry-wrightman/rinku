import { useState, useEffect, useRef, useCallback } from 'react';

export type NodeEventType =
  | 'NewTransaction'
  | 'FastPathConfirmed'
  | 'FastPathExecuted'
  | 'CheckpointCreated'
  | 'AccountUpdated';

export interface NewTransactionData {
  hash: string;
  from: string;
  to: string;
  amount: number;
  kind?: string;
}

export interface FastPathConfirmedData {
  hash: string;
  from: string;
  to: string;
  amount: number;
  total_stake: number;
  threshold: number;
}

export interface FastPathExecutedData {
  hash: string;
  from: string;
  to: string;
  amount: number;
}

export interface CheckpointCreatedData {
  hash: string;
  height: number;
  txs_finalized: number;
  reward: number;
}

export interface AccountUpdatedData {
  address: string;
  balance: number;
  nonce: number;
  staked: number;
}

export type NodeEventData =
  | NewTransactionData
  | FastPathConfirmedData
  | FastPathExecutedData
  | CheckpointCreatedData
  | AccountUpdatedData;

export interface NodeEvent {
  type: NodeEventType;
  data: NodeEventData;
}

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'reconnecting';

export interface UseWebSocketReturn {
  status: ConnectionStatus;
  lastEvent: NodeEvent | null;
  events: NodeEvent[];
  send: (message: string) => void;
}

const MAX_EVENTS = 100;
const MAX_BACKOFF = 30000;
const INITIAL_BACKOFF = 1000;
const HEARTBEAT_INTERVAL = 30000;

function getWsUrl(): string {
  const envApiUrl = import.meta.env.VITE_API_URL;

  if (envApiUrl && !envApiUrl.includes('127.0.0.1') && !envApiUrl.includes('localhost')) {
    const url = new URL(envApiUrl);
    const protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${protocol}//${url.host}/api/ws`;
  }

  if (import.meta.env.PROD) {
    const host = window.location.hostname;
    const wsHost = host.replace(/-5000\./, '-3001.');
    return `wss://${wsHost}/api/ws`;
  }

  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${protocol}//${window.location.host}/api/ws`;
}

export function useWebSocket(): UseWebSocketReturn {
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');
  const [lastEvent, setLastEvent] = useState<NodeEvent | null>(null);
  const [events, setEvents] = useState<NodeEvent[]>([]);
  const wsRef = useRef<WebSocket | null>(null);
  const backoffRef = useRef(INITIAL_BACKOFF);
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const heartbeatTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const mountedRef = useRef(true);

  const clearTimers = useCallback(() => {
    if (reconnectTimerRef.current) {
      clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
    if (heartbeatTimerRef.current) {
      clearInterval(heartbeatTimerRef.current);
      heartbeatTimerRef.current = null;
    }
  }, []);

  const connect = useCallback(() => {
    if (!mountedRef.current) return;

    const url = getWsUrl();
    setStatus('connecting');

    try {
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        if (!mountedRef.current) {
          ws.close();
          return;
        }
        setStatus('connected');
        backoffRef.current = INITIAL_BACKOFF;

        heartbeatTimerRef.current = setInterval(() => {
          if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({ type: 'ping' }));
          }
        }, HEARTBEAT_INTERVAL);
      };

      ws.onmessage = (event) => {
        if (!mountedRef.current) return;
        try {
          const parsed = JSON.parse(event.data) as NodeEvent;
          if (parsed.type && parsed.data) {
            setLastEvent(parsed);
            setEvents((prev) => {
              const next = [parsed, ...prev];
              return next.length > MAX_EVENTS ? next.slice(0, MAX_EVENTS) : next;
            });
          }
        } catch {
        }
      };

      ws.onclose = () => {
        if (!mountedRef.current) return;
        wsRef.current = null;
        clearTimers();
        setStatus('reconnecting');

        const delay = backoffRef.current;
        backoffRef.current = Math.min(backoffRef.current * 2, MAX_BACKOFF);

        reconnectTimerRef.current = setTimeout(() => {
          if (mountedRef.current) {
            connect();
          }
        }, delay);
      };

      ws.onerror = () => {
        if (ws.readyState !== WebSocket.CLOSED) {
          ws.close();
        }
      };
    } catch {
      setStatus('reconnecting');
      const delay = backoffRef.current;
      backoffRef.current = Math.min(backoffRef.current * 2, MAX_BACKOFF);
      reconnectTimerRef.current = setTimeout(() => {
        if (mountedRef.current) {
          connect();
        }
      }, delay);
    }
  }, [clearTimers]);

  const send = useCallback((message: string) => {
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      wsRef.current.send(message);
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    connect();

    return () => {
      mountedRef.current = false;
      clearTimers();
      if (wsRef.current) {
        wsRef.current.onclose = null;
        wsRef.current.close();
        wsRef.current = null;
      }
    };
  }, [connect, clearTimers]);

  return { status, lastEvent, events, send };
}
