import React, { useState, useEffect } from "react";
import { commands, events } from "./bindings";
import type { ChatMessage, SessionHandle } from "./bindings";
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

function App() {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [apiKeyError, setApiKeyError] = useState<string | null>(null);
  const [session, setSession] = useState<SessionHandle | null>(null);
  const [pendingMessages, setPendingMessages] = useState<Set<string>>(
    new Set(),
  );

  useEffect(() => {
    checkApiKey();
    createSession();

    const unlisten = Promise.all([
      events.messagePending.listen((event) => {
        const message: ChatMessage = event.payload.message;
        setMessages((prev) => [...prev, message]);
        setPendingMessages((prev) => new Set(prev).add(message.id));
      }),

      events.messageResponse.listen((event) => {
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
        const message_id = event.payload.message_id;
        const error = event.payload.error;
        setApiKeyError(error);
        setPendingMessages(new Set());
      }),
    ]);

    return () => {
      unlisten.then((unlisteners) => unlisteners.forEach((u) => u()));
    };
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
              className={`message ${message.role} ${pendingMessages.has(message.id) ? "pending" : ""}`}
            >
              {message.content}
              {pendingMessages.has(message.id) && (
                <span className="pending-indicator">...</span>
              )}
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
