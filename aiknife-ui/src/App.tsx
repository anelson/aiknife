import React, { useState, useEffect } from "react";
import { commands, SessionHandle } from "./bindings";
import "./App.css";
import type { ChatMessage } from "./bindings";

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
      const response = await commands.sendMessage(session, userInput);
      setMessages(prevMessages => [...prevMessages, response.user_message, response.assistant_message]);
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
            <div key={message.id} className={`message ${message.role}`}>
              {message.content}
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
            tooltip={
              apiKeyError || 
              (!session ? "Creating session..." : "")
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
