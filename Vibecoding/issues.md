When a book (text) is generated, it says for the chaptors: pending, but does not seme to do anything.
When I then press on Re-write chapters, I get "Could not queue chapter generation" and it generates the text.
When I then press narrate, I get "Could not queue audio generation" and it generates the audio.

When a audiobook starts generating, I see no progress bar, it is not clear it is generating at that time.

The audio generated does not match the text in the chapters. Why is that ?.


Somtimes when generating the artwork, I get following error: upstream service error: openrouter: response had neither text nor image


The image llm list from openrouter is not showing all the models, use the openrouter api to get all the models.
https://openrouter.ai/api/v1/models?output_modalities=image
Also get the correct price for the image models from the api. (input/prompt and output/completion)

I selected the black forest: flux.2 klein 4b as image gen, but it gives an error: upstream service error: openrouter returned 404 Not Found: {"error":{"message":"No endpoints found that support the requested output modalities: image, text","code":404}}

When generating text or images through openrouter, can you get the actual cost a prompt took ? And store that in the database ?
This will enable us to show the user the cost of the generation.