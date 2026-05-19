Create a new branch for a multi audiobook queue system.

The current system only allows one audiobook to be generated at a time.
We want to allow multiple audiobooks to be generated in sequence.

On the audiobook creation page, add an option to add the audiobook to the queue.

Add a new queue follow up page where the user can see the queue and manage it.
- start generating the next audiobook when the current one is done
- pause the queue
- resume the queue
- cancel the queue
- see the status of each audiobook in the queue

Foe each item in the queue, show:
- title
- status
- progress (in what step)
- current cost
- actions (pause, resume, cancel)
- access to a detailed log page


Add the ability to see the details of the audiobook (detail button on the queue item)
This detail page should show the same information as the current audiobook detail page.
Add the ability to remove items from the queue.

a queueitem in progress/running should clearly update the progress bar/percentage and the step info.
Currently it does not update