#!/bin/bash
#
# Download the OpenAI tokenizer token data files from the OpenAI sources.
#
# The URLs were obtained from the `openai_public.py` file in the `tiktoken` repo.
#
# This only is needed if the files are updated, which is rather unlikely since the tokenizer used to train a model must
# exactly match the tokenizer used to tokenize queries against the model.

# Get the directory path where this script is located
SCRIPT_DIR=$(dirname "$(readlink -f "$0")")

mkdir -p "${SCRIPT_DIR}/gpt-2"
curl https://openaipublic.blob.core.windows.net/gpt-2/encodings/main/vocab.bpe -o "${SCRIPT_DIR}/gpt-2/vocab.bpe"
curl https://openaipublic.blob.core.windows.net/gpt-2/encodings/main/encoder.json -o "${SCRIPT_DIR}/gpt-2/encoder.json"

mkdir -p "${SCRIPT_DIR}/r50k_base"
curl https://openaipublic.blob.core.windows.net/encodings/r50k_base.tiktoken -o "${SCRIPT_DIR}/r50k_base/r50k_base.tiktoken"

mkdir -p "${SCRIPT_DIR}/p50k_base"
curl https://openaipublic.blob.core.windows.net/encodings/p50k_base.tiktoken -o "${SCRIPT_DIR}/p50k_base/p50k_base.tiktoken"

mkdir -p "${SCRIPT_DIR}/cl100k_base"
curl https://openaipublic.blob.core.windows.net/encodings/cl100k_base.tiktoken -o "${SCRIPT_DIR}/cl100k_base/cl100k_base.tiktoken"

