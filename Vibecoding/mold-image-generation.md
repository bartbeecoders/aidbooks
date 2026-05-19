in the mold folder ther is an implementation of image generation using [https://github.com/utensils/mold](https://github.com/utensils/mold).

This is a rust based cli tool that can generate images using various models.

Look at the examples script in the mold folder to see how to use it.

Integrate it into the project to generate images for the chapters.

First give me some architecture suggestions on how to integrate it.


I want to move the whole mold implmentation to a seperate service, that I can start seperately and communicate with it via http.
Add a new folder called "mold-service" to the project.
Add test scripts to test the service.