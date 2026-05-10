Endpoints:
    
    | Method | Path | What it does |
    |--------|------|-------------|
    | GET | /api/health | Health check |
    | POST | /api/compositions | Create composition (default template or custom HTML) |
    | GET | /api/compositions | List all compositions |
    | GET | /api/compositions/<id> | Get composition details + HTML |
    | PUT | /api/compositions/<id> | Update composition metadata/HTML |
    | DELETE | /api/compositions/<id> | Delete composition + render |
    | POST | /api/render/<id> | Start rendering (async, returns immediately) |
    | GET | /api/status/<id> | Check render status (rendering/completed/failed) |
    | GET | /api/download/<id> | Download the MP4 file |
    
    Quick example — create and render from your computer:
    
    bash
    Create a composition with default template
    curl -X POST https://hyperframes.hideterms.com/api/compositions \
      -H "Content-Type: application/json" \
      -d '{"id":"myvideo","title":"Hello World","subtitle":"From my API","duration":10}'
    
    Start rendering
    curl -X POST https://hyperframes.hideterms.com/api/render/myvideo
    
    Check status
    curl https://hyperframes.hideterms.com/api/status/myvideo
    
    Download the MP4
    curl -o myvideo.mp4 https://hyperframes.hideterms.com/api/download/myvideo
    
    
    Custom HTML example:
    
    bash
    curl -X POST https://hyperframes.hideterms.com/api/compositions \
      -H "Content-Type: application/json" \
      -d '{"id":"custom","html":"<!doctype html>...your full HyperFrames HTML...","duration":15}'
    
    
    The default template generates a nice gradient title with GSAP fade-in/fade-out animations. You can override with any full HyperFrames-compatible HTML (with data-composition-id, data-start, data-duration, data-track-index attributes and window.__timelines registration).


Use an api_key to access it:
Authorization: Bearer <HYPERFRAMES_API_KEY> header (recommended)
The key is stored in the .env file


The hyperframes video looks good but is fairly simple. Can we not make it more related to the content of the audiobook?
Show more steps, interlaced with the generated images/illustrations.
Maybe let the user set the nr of steps/animations to show.

Add the hyperframes illustration capability to audiobook youtube generation (not only shorts)
Add the option to add it to the new audiobook generation flow.
As youtube videos are longer, we should show more steps and illustrations. Illustrations are not only text overlays, but can also be visual representations of the content.