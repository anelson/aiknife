import React, { useState, useEffect } from "react";
import { commands, events } from "./bindings";
import type { ChatMessage, SessionHandle, Role, UiError } from "./bindings";
import "./App.css";
import {
  warn,
  debug,
  trace,
  info,
  error,
  attachConsole,
} from "@tauri-apps/plugin-log";

interface TooltipButtonProps {
  disabled: boolean;
  tooltip: string;
  onClick: (e: React.MouseEvent<HTMLButtonElement>) => void;
  children: React.ReactNode;
}

const TooltipButton: React.FC<TooltipButtonProps> = ({
  disabled,
  tooltip,
  onClick,
  children,
}) => (
  <div className="tooltip-container">
    <button type="submit" disabled={disabled} onClick={onClick}>
      {children}
    </button>
    {disabled && tooltip && <span className="tooltip">{tooltip}</span>}
  </div>
);

interface MessageState {
  messages: ChatMessage[];
  streamingContent: Record<string, string>;
}

const handleError = (e: unknown) => {
  error("Error details:" + JSON.stringify(e, null, 2));

  const uiError = e as UiError;
  return uiError.details
    ? `${uiError.type}: ${uiError.message}\n\nDetails:\n${uiError.details}`
    : `${uiError.type}: ${uiError.message}`;
};

function useMessageEvents(): MessageState {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [streamingContent, setStreamingContent] = useState<Record<string, string>>({});

  useEffect(() => {
    let mounted = true;

    const setupListeners = async () => {
      const unlisteners = await Promise.all([
        events.newMessage.listen((event) => {
          if (!mounted) return;
          const message: ChatMessage = event.payload.message;
          setMessages((prev) => [...prev, message]);
        }),

        events.messageStatusChanged.listen((event) => {
          if (!mounted) return;
          const { message_id, status } = event.payload;
          setMessages((prev) =>
            prev.map((msg) =>
              msg.id === message_id ? { ...msg, status } : msg
            )
          );
        }),

        events.messageStream.listen((event) => {
          if (!mounted) return;
          const { message_id, message_fragment } = event.payload;
          setStreamingContent((prev) => ({
            ...prev,
            [message_id]: (prev[message_id] || "") + message_fragment,
          }));
        }),
      ]);

      return () => unlisteners.forEach((u: () => void) => u());
    };

    const cleanup = setupListeners();
    return () => {
      mounted = false;
      cleanup.then((cleanupFn) => cleanupFn?.());
    };
  }, []);

  return { messages, streamingContent };
}

interface ErrorMessageProps {
  error: UiError;
}

const ErrorMessage: React.FC<ErrorMessageProps> = ({ error }) => {
  const [isExpanded, setIsExpanded] = useState(false);

  const copyToClipboard = async () => {
    try {
      await navigator.clipboard.writeText(
        `${error.type}: ${error.message}${error.details ? `\n\n${error.details}` : ""}`,
      );
    } catch (err) {
      console.error("Failed to copy error to clipboard:", err);
    }
  };

  if (!error) return null;

  return (
    <div className="relative inline-block">
      <span
        className="error-trigger text-red-500 cursor-pointer hover:opacity-80"
        onClick={(e) => {
          e.stopPropagation();
          setIsExpanded(!isExpanded);
        }}
        title="Click to show error details"
      >
        ⚠️
      </span>
      {isExpanded && (
        <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50">
          <div className="bg-red-50 rounded-lg shadow-lg w-full max-w-4xl overflow-hidden">
            <div className="p-6 space-y-4 text-left">
              <div className="font-mono text-red-700 text-lg font-bold">
                {error.type}: {error.message}
              </div>

              {error.details && (
                <div className="font-mono text-red-900 whitespace-pre-wrap">
                  {error.details}
                </div>
              )}
            </div>

            <div className="border-t border-red-200 p-4 flex justify-end gap-2">
              <button
                onClick={copyToClipboard}
                className="bg-red-100 text-red-700 hover:bg-red-200 text-sm px-3 py-1 rounded flex items-center gap-1"
                title="Copy error details to clipboard"
              >
                Copy
              </button>
              <button
                onClick={() => setIsExpanded(false)}
                className="bg-red-100 text-red-700 hover:bg-red-200 text-sm px-3 py-1 rounded"
              >
                Close
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

function App() {
  const { messages, streamingContent } = useMessageEvents();
  const [input, setInput] = useState("");
  const [chatSessionError, setChatSessionError] = useState<string | null>(null);
  const [session, setSession] = useState<SessionHandle | null>(null);

  useEffect(() => {
    checkApiKey();
    createSession();

    // Wire up the Tauri log output to the console for easier debugging
    const setupLogger = async () => {
      const detach = await attachConsole();
      return detach;
    };
    const detachPromise = setupLogger();

    // Cleanup when component unmounts
    return () => {
      detachPromise.then((detach) => detach());
    };
  }, []);

  const handleSessionError = (error: unknown) => {
    const formattedError = handleError(error);
    setChatSessionError(formattedError);
  };

  const checkApiKey = async () => {
    try {
      await commands.checkApiKey();
      setChatSessionError(null);
    } catch (error) {
      handleSessionError(error);
    }
  };

  const createSession = async () => {
    try {
      const newSession = await commands.newSession();
      setSession(newSession);
    } catch (error) {
      handleSessionError(error);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!input.trim() || chatSessionError || !session) return;

    const userInput = input.trim();
    setInput("");

    try {
      await commands.sendMessage(session, userInput);
    } catch (error) {
      handleSessionError(error);
    }
  };

  const handleAbort = async (messageId: string) => {
    try {
      await commands.abortMessage(messageId);
    } catch (error) {
      handleSessionError(error);
    }
  };

  const handleRetry = async (messageId: string, content: string) => {
    if (!session) return;

    try {
      await commands.retryMessage(session, messageId, content);
    } catch (error) {
      handleSessionError(error);
    }
  };

  const isMessageInProgress = (message: ChatMessage) => {
    return (
      message.status.type === "Processing" || 
      message.status.type === "Streaming"
    );
  };

  return (
    <div className="App">
      {chatSessionError && (
        <div className="error-banner">
          <p>{chatSessionError}</p>
        </div>
      )}
      <div className="app-content">
        <h1>Simple ChatGPT Clone</h1>
        <div className="chat-container">
          {messages.map((message) => (
            <div
              key={message.id}
              className={`message ${message.role.toLowerCase()}`}
            >
              <div className="flex items-start gap-2">
                <span className="flex-grow">
                  {streamingContent[message.id] || message.content}
                </span>
                {message.status.type === "Error" && (
                  <>
                    <ErrorMessage error={message.status.error} />
                    <button
                      onClick={() => handleRetry(message.id, message.content)}
                      className="retry-button"
                    >
                      Retry
                    </button>
                  </>
                )}
                {isMessageInProgress(message) && streamingContent[message.id] && (
                  <button
                    onClick={() => handleAbort(message.id)}
                    className="abort-button"
                  >
                    Stop
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
        <form onSubmit={handleSubmit}>
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="Type your message..."
            disabled={!!chatSessionError || !session}
          />
          <TooltipButton
            disabled={!!chatSessionError || !session || !input.trim()}
            tooltip={
              chatSessionError || (!session ? "Creating session..." : "")
            }
            onClick={(e) => {
              e.preventDefault();
              handleSubmit(e);
            }}
          >
            Send
          </TooltipButton>
        </form>
      </div>
    </div>
  );
}

export default App;
