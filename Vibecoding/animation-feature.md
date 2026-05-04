I want to implment a full animation feature. It should created, based on the audiobook, a animation supporting the text. This would then be part of the youtube video.

Research this idea and look for good open source animation tools that can be used to create the animation.
Look at Motion Canvas, Reanimate, and Noon.
ALso reseach other tools and compare them.

Make a detailed plan


On the animate feature, create a smaller test demo scripts that only creates a small animation for one chapter.
I want to see if the animation tool works well and if it can be used for the full animation feature.


The animate feature is way to slow. We need to find a way to speed it up, or take a different approach.
Can you investigate if it makes sense to use x.ai grok video to generate the animation? (see https://docs.x.ai/developers/model-capabilities/video/generation)
Process could be:
1. Extract the text from the audiobook
2. Split it into paragraphs or sections that make sense
3. Generate a start illustration for each section
4. Use x.ai grok video to generate the animation (starting with the illustration)
5. Combine the animation with the audiobook

Investigate if this approach is viable and if it can be used for the full animation feature.


Add the possibility to redo the animation for a specific chapter.
I'm also the animation is more graphical, so it might be better to use a different approach.
What about starting with the illustrations and then go in to a graphical view of the topic ?
This could maybe work better with physics, science and math topics.

Implement the capability to set a seperate LLM to define the manim code for the animation.
This way I can assign a more specialized LLM to define the manim code for the animation.

I cannot see where to indicate that the llm is used for manim animation.
Maybe add a page on the admin setting for Manim animation, (and the other animation tools as well) to indicate which llm is used for each tool. We can also use it to maintain other animation settings.

When doing the animate (all chapters) feature, make sure to use the llm that is set for manim animation and include the classify step for each chapter so that the diagrams are generated correctly.

To better test different LLm's with the animate feature, add a LLM test button on a chapter page that allows to test the animation with a specific LLM.
This should show the prompt used, and the prompt result, cost and time taken.