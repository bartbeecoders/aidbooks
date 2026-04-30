When playing back an audiobook, it would be nice to have a progress bar that shows the current position in the audiobook.
Show the text of the current chapter below the progress bar.

When creating a new audiobook:
- make the genre a editable dropdown list, use icons/images for the genres
- add the possibility to select the x.ai voice


Add the possibility to generate coverart.
- add to the settings page the option to select a LLM model for art/image generation. This uses the openrouter api.
- add to the new audiobook page a button to generate a coverart based on the topic and genre


Add the option to generate the audiobook in a different language.
- add to the new audiobook page a dropdown to select the language (English is default)
- add English, Dutch, French, German, Spanish, Italian, Portuguese, Russian, Chinese, Japanese, Korean

In the audiobook detail page, show the language of the audiobook, show the coverart, and show the voice used for the audiobook.
Add the ability to regenerate the coverart.
Add the ability to generate the audiobook in a different language, translate the text and regenerate the audiobook. Multiple languages are then available to the user.
Add the ability to change the voice used for the audiobook.

Implement topic templates.

Let me define topic prompts and store them in the database.
Use the admin panel to add, edit, and delete topic prompts.
On the new audiobook page, show a dropdown to select a topic template. This is off course editable.

Introduce the option to set the style of the artwork.
Like realistic, cartoon, abstract, etc.

Add the ability to edit the LLM list, update costs, update data, remove.
Add the ability to assign the main function of an LLM (e.g. text generation, image generation, etc.)
Add the ability to assign languages to an LLM (e.g. English, Dutch, French, German, Spanish, Italian, Portuguese, Russian, Chinese, Japanese, Korean). Can be multiple languages.
Add the ability to set LLM priority (e.g. 1, 2, 3, etc.)

The coverart size/dimensions does not fit if we publish the audiobook to youtube.

When generating coverart, show a dropdown when multiple LLM's are available for art generation.

Improve the jobs page, allow for canceling jobs.
Allow to remove jobs from the jobs page.

Todo:
Add areview capability for the youtube video before publishing.

Add the option to:
Publish the audiobook as a playlist on youtube. Each chapter should be a video in the playlist.

Add in the admin page a seperate page for Image LLM's. (for the cover art)
Pricing is $ per megapixel.

To add new LLM's, use the openrouter.ai endpoint to get the list of available models, see https://openrouter.ai/docs/guides/overview/models to get the list.

On the audiobook detail page, put the logging cards in a log tab, so I can check them if needed, but theyt do not take up space

When translating, give more feedback in the progress card, like what chapter is being translated and what language it's being translated to.


When multiple languages are available (text + audio), publish the audiobook to youtube with the capability of selecting the language in youtube.


Add an option to generate an audiobook in 1 pass:
- Generate the text
- Generate the coverart
- Generate the audiobook
- Publish the audiobook to youtube

Add this as an option list to the "New audiobook" dialog box. Also make the "new audio book" dialog box wider, put more options next to each other. Make the "new audio book" generation flow the default.

The pipeline creates the chapters, the main coverart but not the narrate and the coverart for each chapter.
It seems to stop after the chapters are created. Investigate why the narration and chapter coverart are not being generated.
Also each of these steps in the pipeline should shown as a step in the activity log (cards on screen), so the user can follow along.


Add an option to add more artwork to the audiobook, like images that we can show during the audiobook playback. They need to be relevent to the content of the audiobook.


## X.ai LLM api
Add to the LLM dialog a tab that enables us to add a model from x.ai: https://docs.x.ai/developers/model-capabilities/text/generate-text
So the add LLM dialog has 2 tabs, 1 for openrouter.ai and 1 for x.ai (later we will add other providers)

When I generate text, the system does not use the highest priotity LLM, it always uses the openrouter anthropic/claude-haiku-4.5


## X.ai image LLM api
Add to the image LLM dialog a tab that enables us to add a model from x.ai: https://docs.x.ai/developers/model-capabilities/images/generation
So the add LLM dialog has 2 tabs, 1 for openrouter.ai and 1 for x.ai (later we will add other providers)

When I generate text, the system does not use the highest priotity LLM, it always uses the openrouter anthropic/claude-haiku-4.5




## language improvments
Put the language selection above the template selection in the "New audiobook" dialog box.
The topic (with surprise factor) should be generated in the selected language.
Use the llm model with the highest priority for the selected language.
Show as an indication the LLM model that will be used for the selected language. Put this next to the language selection.
When generating the audiobook with the pipeline, use the selected language and the selected LLM model for that language.

When generating Dutch (a non english book), the pipeline does not seem to write the dutch text. It shows Dutch (primary), with no chapters, and English with the dutch chapters.

For a audiobook that has multiple languages, how is it uploaded to youtube. If all languages are uploaded how do I select the language in youtube ?


## cost improvements

X.ai text to speech = $4.2 per 1M characters
X.ai chat prompts = $2.0 per 1M tokens
The X.ai prompt returns the token usage, so we can track the cost of each prompt.
"usage": {
  "prompt_tokens": 199,
  "completion_tokens": 1,
  "total_tokens": 200,
  "prompt_tokens_details": {
    "text_tokens": 199,
    "audio_tokens": 0,
    "image_tokens": 0,
    "cached_tokens": 163
  },
  "completion_tokens_details": {
    "reasoning_tokens": 0,
    ...
  }
}


### LLM priority change
Allow in the UI for drag and drop to change the priority of the LLM models. (text LLMs and image LLMs)

### Youtube upload improvements
Add to the admin settings a page where we can configure the youtube upload settings.
I want to be able to add a standard text to each of youtube descriptions. This will include a disclaimer and a link to the website.
This text should be able to be set in each language. 
This text is then added to the end of the description when uploading to youtube.

### Library browsing improvements
Can we combine the library browsing with the audiobook detail page? So when we click on an audiobook, we see the detail page with the chapters and the ability to play the audiobook.
Left pane is the list of audiobooks, right pane is the detail page.
Add a filter to the left pane to filter the audiobooks by status.
Add sorting to the left pane to sort the audiobooks by title, language, or status.
Add a category to the audiobook, so we can group books by category in the list.



