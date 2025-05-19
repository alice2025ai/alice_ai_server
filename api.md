# Alice AI Server API Documentation

## 1. Signature Verification

### Verify Signature

- **URL**: `/verify-signature`
- **Method**: POST
- **Description**: Verify user signature and authorize user to send messages in Telegram group
- **Request Body**:
  ```json
  {
    "challenge": "string",
    "chat_id": "string",
    "signature": "string",
    "user": "string",
    "chain_type": "string" (optional, default is "monad")
  }
  ```
- **Response**:
  ```json
  {
    "success": true|false,
    "error": "string" (optional)
  }
  ```
- **Notes**: 
  - Verifies if the user signature is valid
  - Checks if the user owns project shares
  - If they have shares, grants the user permission to speak in the Telegram group

## 2. Agent Management

### Add Telegram Bot

- **URL**: `/add_tg_bot`
- **Method**: POST
- **Description**: Add a new Telegram bot
- **Request Body**:
  ```json
  {
    "bot_token": "string",
    "chat_group_id": "string",
    "subject_address": "string",
    "agent_name": "string",
    "invite_url": "string",
    "bio": "string" (optional)
  }
  ```
- **Response**:
  ```json
  {
    "success": true|false,
    "error": "string" (optional)
  }
  ```

### Get Agent List

- **URL**: `/agents`
- **Method**: GET
- **Description**: Get a list of all agents, with pagination support
- **Query Parameters**: 
  - `page`: Page number (default: 1)
  - `page_size`: Items per page (default: 10)
- **Response**:
  ```json
  {
    "agents": [
      {
        "agent_name": "string",
        "subject_address": "string",
        "created_at": "string" (ISO format time)
      }
    ],
    "total": 0,
    "page": 0,
    "page_size": 0
  }
  ```

### Get Agent by Name

- **URL**: `/agents/{agent_name}`
- **Method**: GET
- **Description**: Get agent information by agent name
- **Path Parameters**: 
  - `agent_name`: Agent name
- **Response**:
  ```json
  {
    "agent": {
      "agent_name": "string",
      "subject_address": "string",
      "created_at": "string" (ISO format time)
    },
    "success": true|false,
    "error": "string" (optional)
  }
  ```

### Get Agent Details

- **URL**: `/agent/detail/{agent_name}`
- **Method**: GET
- **Description**: Get detailed information for an agent
- **Path Parameters**: 
  - `agent_name`: Agent name
- **Response**:
  ```json
  {
    "agent_name": "string",
    "subject_address": "string",
    "invite_url": "string",
    "bio": "string" (optional),
    "success": true|false,
    "error": "string" (optional)
  }
  ```

## 3. User Information

### Get User Shares

- **URL**: `/users/{user_address}/shares/{chain_type}`
- **Method**: GET
- **Description**: Get all shares owned by a user on a specific blockchain
- **Path Parameters**:
  - `user_address`: User address
  - `chain_type`: Blockchain type
- **Response**:
  ```json
  {
    "user_address": "string",
    "shares": [
      {
        "subject_address": "string",
        "shares_amount": "string"
      }
    ],
    "chain_type": "string"
  }
  ```