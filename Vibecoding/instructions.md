# ListenAI - AI Audiobook Generator

## Project Overview

A web application and iOS app that allows users to generate audiobooks using AI based on topics they provide. The system will create structured content and convert it to natural-sounding speech.

## Core Features

### 1. Topic Input & Content Generation
- User provides a topic, subject, or theme
- AI generates structured audiobook content (chapters, sections)
- Options for content length (short, medium, long)
- Genre/style selection (educational, narrative, conversational,etc)

### 2. AI topic generation
- System can call an LLM to generate random topics, set genre and length

### 3. Voice & Audio Generation
- Text-to-speech conversion using AI voice synthesis
- Multiple voice options (different accents, genders, tones)
- Adjustable speaking pace
- Background music/ambient sound options (optional)

### 4. User Library
- Save generated audiobooks
- Playback controls (play, pause, skip, speed adjustment)
- Progress tracking and bookmarks
- Download for offline listening

### 5. User Accounts
- Authentication (email, social login)
- Usage tracking and limits (free tier vs premium)
- History of generated content

### 6. Admin Panel
- Manage LLM list
- Manage voices
- Manage users
- Manage content

### 7. Overall UI guide
- light/dark mode
- responsive design
- modern look and feel
- use the flux2 skill for images


## Technical Architecture

### Frontend - Web
- **Framework**: React / vite / Typescript
- **Styling**: Tailwind CSS & shadcn/ui & Radix UI
- **Audio Player**: Custom player with wavesurfer.js or similar
- **State Management**: Zustand or Redux
- **Routing**: tanstack router
Put frontend in /frontend

### Backend
- **Runtime**: Rust 
- **API**: REST Api
- **Database**: SurrealDB (embedded database)
- **Storage**: Local filesystem for audio files
Put backend in /backend


### AI Services
- **Content Generation**: OpenRouter.ai (access to multiple LLMs)
    - LLM list , selection and settings to be stored in the database, 
    - admin user can manage LLM List

- **Text-to-Speech**: x.ai api
    - see https://docs.x.ai/developers/model-capabilities/audio/voice-agent

## User Flow

1. **Landing Page** → Sign up / Log in
2. **Dashboard** → View library, create new audiobook
3. **Create Flow**:
   - Enter topic/subject
   - Select length and style
   - Choose voice
   - Generate (shows progress)
   - Preview and save
4. **Library** → Browse, play, manage audiobooks
5. **Player** → Full playback experience with controls

