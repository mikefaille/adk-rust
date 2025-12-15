import { useCallback, useRef, useState } from 'react';

interface ToolCall {
  name: string;
  args: unknown;
}

export function useSSE(projectId: string | null, binaryPath?: string | null) {
  const [isStreaming, setIsStreaming] = useState(false);
  const [streamingText, setStreamingText] = useState('');
  const [currentAgent, setCurrentAgent] = useState('');
  const [toolCalls, setToolCalls] = useState<ToolCall[]>([]);
  const esRef = useRef<EventSource | null>(null);
  const textRef = useRef('');

  const send = useCallback(
    (input: string, onComplete: (text: string) => void, onError?: (msg: string) => void) => {
      if (!projectId) return;

      textRef.current = '';
      setStreamingText('');
      setCurrentAgent('');
      setToolCalls([]);
      setIsStreaming(true);

      const params = new URLSearchParams({ input });
      if (binaryPath) {
        params.set('binary_path', binaryPath);
      }
      const es = new EventSource(`/api/projects/${projectId}/stream?${params}`);
      esRef.current = es;
      let ended = false;

      es.addEventListener('agent', (e) => {
        if (textRef.current) {
          textRef.current += '\n\n';
          setStreamingText(textRef.current);
        }
        setCurrentAgent(e.data);
      });

      es.addEventListener('chunk', (e) => {
        textRef.current += e.data;
        setStreamingText(textRef.current);
      });

      es.addEventListener('tool_call', (e) => {
        try {
          const data = JSON.parse(e.data);
          setToolCalls(prev => [...prev, { name: data.name, args: data.args }]);
          textRef.current += `\n🔧 Calling ${data.name}...\n`;
          setStreamingText(textRef.current);
        } catch {}
      });

      es.addEventListener('tool_result', (e) => {
        try {
          const data = JSON.parse(e.data);
          const resultStr = typeof data.result === 'string' ? data.result : JSON.stringify(data.result).slice(0, 200);
          textRef.current += `✓ ${data.name}: ${resultStr}\n`;
          setStreamingText(textRef.current);
        } catch {}
      });

      es.addEventListener('end', () => {
        ended = true;
        const finalText = textRef.current;
        setStreamingText('');
        setCurrentAgent('');
        setIsStreaming(false);
        es.close();
        onComplete(finalText);
      });

      es.addEventListener('error', (e) => {
        if (!ended) {
          const msg = (e as MessageEvent).data || 'Connection error';
          setStreamingText('');
          setCurrentAgent('');
          setIsStreaming(false);
          es.close();
          onError?.(msg);
        }
      });
    },
    [projectId]
  );

  const cancel = useCallback(() => {
    esRef.current?.close();
    setStreamingText('');
    setCurrentAgent('');
    setIsStreaming(false);
  }, []);

  return { send, cancel, isStreaming, streamingText, currentAgent, toolCalls };
}
