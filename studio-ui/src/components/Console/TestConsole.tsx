import { useState, useRef, useEffect } from 'react';
import ReactMarkdown from 'react-markdown';
import { useStore } from '../../store';
import { useSSE } from '../../hooks/useSSE';

interface Message {
  role: 'user' | 'assistant';
  content: string;
  agent?: string;
}

type FlowPhase = 'idle' | 'input' | 'output';

interface Props {
  onFlowPhase?: (phase: FlowPhase) => void;
  binaryPath?: string | null;
}

export function TestConsole({ onFlowPhase, binaryPath }: Props) {
  const { currentProject } = useStore();
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState('');
  const { send, cancel, isStreaming, streamingText, currentAgent, toolCalls } = useSSE(currentProject?.id ?? null, binaryPath);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const sendingRef = useRef(false);
  const lastAgentRef = useRef<string | null>(null);

  useEffect(() => {
    if (currentAgent) lastAgentRef.current = currentAgent;
  }, [currentAgent]);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, streamingText]);

  useEffect(() => {
    if (streamingText) {
      onFlowPhase?.('output');
    } else if (!isStreaming) {
      onFlowPhase?.('idle');
    }
  }, [streamingText, isStreaming, onFlowPhase]);

  const sendMessage = () => {
    if (!input.trim() || !currentProject || isStreaming || sendingRef.current) return;
    sendingRef.current = true;
    const userMsg = input.trim();
    setInput('');
    setMessages((m) => [...m, { role: 'user', content: userMsg }]);
    onFlowPhase?.('input');
    lastAgentRef.current = null;
    
    send(
      userMsg,
      (text) => {
        if (text) {
          setMessages((m) => [...m, { role: 'assistant', content: text, agent: lastAgentRef.current || undefined }]);
        }
        onFlowPhase?.('idle');
        sendingRef.current = false;
      },
      (error) => {
        setMessages((m) => [...m, { role: 'assistant', content: `Error: ${error}` }]);
        onFlowPhase?.('idle');
        sendingRef.current = false;
      }
    );
  };

  const clearChat = async () => {
    if (currentProject) {
      await fetch(`/api/projects/${currentProject.id}/session`, { method: 'DELETE' });
    }
    setMessages([]);
  };

  const handleCancel = () => {
    cancel();
    onFlowPhase?.('idle');
  };

  const isThinking = isStreaming && !streamingText;

  return (
    <div className="flex flex-col h-full bg-studio-panel border-t border-gray-700">
      <div className="p-2 border-b border-gray-700 text-sm font-semibold flex justify-between">
        <span>💬 Test Console</span>
        <div className="flex gap-2">
          {messages.length > 0 && !isStreaming && (
            <button onClick={clearChat} className="text-gray-400 text-xs hover:text-white">Clear</button>
          )}
          {isStreaming && (
            <button onClick={handleCancel} className="text-red-400 text-xs">Stop</button>
          )}
        </div>
      </div>
      <div className="flex-1 overflow-y-auto p-3 space-y-3">
        {messages.length === 0 && !streamingText && !isThinking && (
          <div className="text-gray-500 text-sm">Send a message to test your agent...</div>
        )}
        {messages.map((m, i) => (
          <div key={i} className={`text-sm ${m.role === 'user' ? 'text-blue-400' : 'text-gray-200'}`}>
            <span className="font-semibold">{m.role === 'user' ? 'You: ' : `${m.agent || 'Agent'}: `}</span>
            {m.role === 'user' ? (
              <span>{m.content}</span>
            ) : (
              <div className="prose prose-invert prose-sm max-w-none inline">
                <ReactMarkdown>{m.content}</ReactMarkdown>
              </div>
            )}
          </div>
        ))}
        {isThinking && (
          <div className="text-sm text-gray-400 flex items-center gap-2">
            <span className="animate-spin">⏳</span>
            <span>{currentAgent ? `${currentAgent} is thinking...` : 'Thinking...'}</span>
          </div>
        )}
        {streamingText && (
          <div className="text-sm text-gray-200">
            <span className="font-semibold">{currentAgent || 'Agent'}: </span>
            <div className="prose prose-invert prose-sm max-w-none inline">
              <ReactMarkdown>{streamingText}</ReactMarkdown>
            </div>
            <span className="animate-pulse">▌</span>
          </div>
        )}
        {toolCalls.length > 0 && isStreaming && (
          <div className="text-xs text-yellow-400 mt-1">
            Tools used: {toolCalls.map(t => t.name).join(', ')}
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>
      <div className="p-2 border-t border-gray-700 flex gap-2">
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.repeat) {
              e.preventDefault();
              sendMessage();
            }
          }}
          placeholder="Type a message..."
          className="flex-1 px-3 py-2 bg-studio-bg border border-gray-600 rounded text-sm"
          disabled={isStreaming}
        />
        <button
          onClick={sendMessage}
          disabled={isStreaming || !input.trim()}
          className="px-4 py-2 bg-studio-highlight rounded text-sm disabled:opacity-50"
        >
          Send
        </button>
      </div>
    </div>
  );
}
