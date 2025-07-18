/**
 * Custom semantic-release plugin to skip release if last commit is a back-merge
 */

const { execSync } = require('child_process');

function getLastCommitMessage() {
    try {
        return execSync('git log -1 --pretty=format:"%s"', { encoding: 'utf-8' }).trim();
    } catch (error) {
        console.error('Error getting last commit message:', error);
        return '';
    }
}

function isBackMergeCommit(message) {
    return message.startsWith('chore(release): back-merge');
}

module.exports = {
    analyzeCommits: (pluginConfig, context) => {
        const lastCommitMessage = getLastCommitMessage();

        context.logger.log(`Last commit message: ${lastCommitMessage}`);

        if (isBackMergeCommit(lastCommitMessage)) {
            context.logger.log('Skipping release - last commit is a back-merge');
            process.exit(0); // Exit the process to skip the release
        }

        context.logger.log('Last commit is not a back-merge, proceeding with normal analysis');
        return false; // Continue with normal analysis
    }
};
