"""
Basic Python program that generates the known-answer test data against the official OpenAI tiktoken
implementation.
"""
import sys
import tiktoken

if len(sys.argv) < 3:
    print("Usage: python3 get_known_answers.py <encoding> <filename>")
    sys.exit(1)

encoding = sys.argv[1]
filename = sys.argv[2]

enc = tiktoken.get_encoding(encoding)

with open(filename, 'r') as file:
    content = file.read()
    tokens = enc.encode(content)
    for token in tokens:
        print(token)

    assert (enc.decode(tokens) == content)
