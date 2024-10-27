import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { commands, Message, SessionHandle } from "./bindings";
import "./App.css";

interface TooltipButtonProps {
  disabled: boolean;
  tooltip: string;
  onClick: () => void;
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
    {disabled && <span className="tooltip">{tooltip}</span>}
  </div>
);

function App() {
  const [messages, setMessages] = useState<Message[]>([]);
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

    const userMessage: Message = { role: "user", content: input };
    setMessages((prevMessages) => [...prevMessages, userMessage]);
    setInput("");

    try {
      const response = await commands.sendMessage(session, input);
      const assistantMessage: Message = {
        role: "assistant",
        content: response,
      };
      setMessages((prevMessages) => [...prevMessages, assistantMessage]);
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
          {messages.map((message, index) => (
            <div key={index} className={`message ${message.role}`}>
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
            disabled={!!apiKeyError || !session}
            tooltip={apiKeyError || (!session ? "Creating session..." : "")}
            onClick={handleSubmit}
          >
            {apiKeyError ? "⚠️" : "Send"}
          </TooltipButton>
        </form>
      </div>
    </div>
  );
}

export default App;
