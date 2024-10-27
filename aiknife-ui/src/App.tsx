import React, { useState, useEffect } from 'react';
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

interface Message {
  role: 'user' | 'assistant';
  content: string;
}

interface TooltipButtonProps {
  disabled: boolean;
  tooltip: string;
  onClick: () => void;
  children: React.ReactNode;
}

const TooltipButton: React.FC<TooltipButtonProps> = ({ disabled, tooltip, onClick, children }) => (
  <div className="tooltip-container">
    <button type="submit" disabled={disabled} onClick={onClick}>
      {children}
    </button>
    {disabled && <span className="tooltip">{tooltip}</span>}
  </div>
);

function App() {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState('');
  const [apiKeyError, setApiKeyError] = useState<string | null>(null);

  useEffect(() => {
    checkApiKey();
  }, []);

  const checkApiKey = async () => {
    try {
      await invoke('check_api_key_command');
      setApiKeyError(null); // Clear any previous error if the check succeeds
    } catch (error) {
      setApiKeyError(error as string);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!input.trim() || apiKeyError) return;

    const userMessage: Message = { role: 'user', content: input };
    setMessages([...messages, userMessage]);
    setInput('');

    try {
      const response = await invoke<string>('send_message', {
        messages: [...messages, userMessage],
      });
      const assistantMessage: Message = { role: 'assistant', content: response };
      setMessages((prevMessages) => [...prevMessages, assistantMessage]);
    } catch (error) {
      console.error('Error:', error);
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
            disabled={!!apiKeyError}
          />
          <TooltipButton
            disabled={!!apiKeyError}
            tooltip={apiKeyError || ''}
            onClick={handleSubmit}
          >
            {apiKeyError ? '⚠️' : 'Send'}
          </TooltipButton>
        </form>
      </div>
    </div>
  );
}

export default App;
