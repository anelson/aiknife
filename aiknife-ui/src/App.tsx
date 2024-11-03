import React, { useState, useEffect } from "react";
import { commands, events } from "./bindings";
import type { ChatMessage, SessionHandle, Role } from "./bindings";
import "./App.css";

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
  pendingMessages: Set<string>;
  messageErrors: Record<string, string>;
}

function useMessageEvents(): MessageState {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [pendingMessages, setPendingMessages] = useState<Set<string>>(new Set());
  const [messageErrors, setMessageErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    let mounted = true;

    const setupListeners = async () => {
      const unlisteners = await Promise.all([
        events.messagePending.listen((event) => {
          if (!mounted) return;
          const message: ChatMessage = event.payload.message;
          setMessages((prev) => [...prev, message]);
          setPendingMessages((prev) => new Set(prev).add(message.id));
        }),

        events.messageResponse.listen((event) => {
          if (!mounted) return;
          const message: ChatMessage = event.payload.message;
          setMessages((prev) => [...prev, message]);
          setPendingMessages((prev) => {
            const newSet = new Set(prev);
            const lastPending = Array.from(newSet).pop();
            if (lastPending) newSet.delete(lastPending);
            return newSet;
          });
        }),

        events.messageError.listen((event) => {
          if (!mounted) return;
          const { message_id, error } = event.payload;
          setMessageErrors(prev => ({
            ...prev,
            [message_id]: error
          }));
          setPendingMessages(new Set());
        }),
      ]);
      
      return () => unlisteners.forEach(u => u());
    };

    const cleanup = setupListeners();
    return () => {
      mounted = false;
      cleanup.then(cleanupFn => cleanupFn?.());
    };
  }, []);

  return { messages, pendingMessages, messageErrors };
}

function App() {
  const { messages, pendingMessages, messageErrors } = useMessageEvents();
  const [input, setInput] = useState("");
  const [apiKeyError, setApiKeyError] = useState<string | null>(null);
  const [session, setSession] = useState<SessionHandle | null>(null);

  useEffect(() => {
    checkApiKey();
    createSession();
  }, []);

  const checkApiKey = async () => {
    try {
      await commands.checkApiKey();
      setApiKeyError(null);
    } catch (error) {
      setApiKeyError(error as string);
    }
  };

  const createSession = async () => {
    try {
      const newSession = await commands.newSession();
      setSession(newSession);
    } catch (error) {
      console.error("Error creating session:", error);
      setApiKeyError(error as string);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!input.trim() || apiKeyError || !session) return;

    const userInput = input.trim();
    setInput("");

    try {
      await commands.sendMessage(session, userInput);
    } catch (error) {
      console.error("Error:", error);
      setApiKeyError(error as string);
    }
  };

  return (
    <div className="App">
      {apiKeyError && (
        <div className="error-banner">
          <p>{apiKeyError}</p>
        </div>
      )}
      <div className="app-content">
        <h1>Simple ChatGPT Clone</h1>
        <div className="chat-container">
          {messages.map((message) => (
            <div
              key={message.id}
              className={`message ${message.role.toLowerCase()} ${
                pendingMessages.has(message.id) ? "pending" : ""
              } ${messageErrors[message.id] ? "tooltip-container" : ""}`}
            >
              <div className="flex items-center gap-2">
                <span>{message.content}</span>
                {messageErrors[message.id] && (
                  <>
                    <span className="text-red-500">⚠️</span>
                    <span className="error-tooltip">{messageErrors[message.id]}</span>
                  </>
                )}
                {pendingMessages.has(message.id) && (
                  <span className="pending-indicator">...</span>
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
            disabled={!!apiKeyError || !session}
          />
          <TooltipButton
            disabled={!!apiKeyError || !session || !input.trim()}
            tooltip={apiKeyError || (!session ? "Creating session..." : "")}
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
